use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{ReviewMode, WorkerRole};
use pip_controller::{
    DispatchContext, DispatchError, ExecutionKind, RolePolicy, WorkflowPolicy,
    schedule_claimed_dispatch, schedule_effect,
};
use pip_core::{
    CaseId, Effect, GitSha, IssueNumber, PlanVersion, PullRequestNumber, RepositoryId,
    StateRevision, WorkflowVersion,
};
use pip_store::{ClaimedEffect, StoredCase};
use serde_json::json;

fn role(
    role: WorkerRole,
    profile: &str,
    execution: ExecutionKind,
    provider: &str,
    model: &str,
    max_runtime: &str,
) -> RolePolicy {
    RolePolicy {
        role,
        reviewer_id: None,
        review_mode: None,
        profile: profile.into(),
        execution,
        provider: provider.into(),
        model: model.into(),
        max_runtime: max_runtime.into(),
        priority: 10,
        skills: vec!["workflow-contract".into(), profile.into()],
    }
}

fn reviewer(
    reviewer_id: &str,
    worker_role: WorkerRole,
    profile: &str,
    execution: ExecutionKind,
    provider: &str,
    model: &str,
    mode: ReviewMode,
) -> RolePolicy {
    let mut configured = role(worker_role, profile, execution, provider, model, "30m");
    configured.skills = vec![
        "workflow-contract".into(),
        match worker_role {
            WorkerRole::ReviewerGeneral => "reviewer-general",
            WorkerRole::ReviewerSecperf => "reviewer-secperf",
            _ => panic!("reviewer role required"),
        }
        .into(),
    ];
    RolePolicy {
        reviewer_id: Some(reviewer_id.into()),
        review_mode: Some(mode),
        ..configured
    }
}

fn policy() -> WorkflowPolicy {
    WorkflowPolicy::new(
        "pip-mdk",
        "/var/lib/pip/worktrees/mdk",
        "pip/",
        vec![
            role(
                WorkerRole::Planner,
                "planner",
                ExecutionKind::Hermes,
                "openai-codex",
                "gpt-5.6-sol",
                "30m",
            ),
            role(
                WorkerRole::Builder,
                "builder-grok",
                ExecutionKind::Direct,
                "cursor",
                "cursor-grok-4.6-high-fast",
                "60m",
            ),
            reviewer(
                "general-sol",
                WorkerRole::ReviewerGeneral,
                "reviewer-general",
                ExecutionKind::Hermes,
                "openai-codex",
                "gpt-5.6-sol",
                ReviewMode::Required,
            ),
            reviewer(
                "secperf-kimi",
                WorkerRole::ReviewerSecperf,
                "reviewer-secperf",
                ExecutionKind::Direct,
                "cursor",
                "kimi-k3-max",
                ReviewMode::Required,
            ),
            reviewer(
                "secperf-opus",
                WorkerRole::ReviewerSecperf,
                "reviewer-secperf-opus",
                ExecutionKind::Direct,
                "cursor",
                "claude-opus-5-thinking-high",
                ReviewMode::Shadow,
            ),
            role(
                WorkerRole::FinalReviewer,
                "final-reviewer",
                ExecutionKind::Hermes,
                "openai-codex",
                "gpt-5.6-sol",
                "30m",
            ),
        ],
    )
    .unwrap()
}

#[test]
fn reauthorized_jobs_focus_the_new_issue_context() {
    let mut ctx = context();
    ctx.immutable_evidence_bundle["records"] = json!({"events":[
        {"event_type":"ISSUE_AUTHORIZED","payload_sha256":"old-context"},
        {"event_type":"ISSUE_REAUTHORIZED","payload_sha256":"fresh-context"}
    ]});
    let jobs = schedule_effect("reauth", Effect::DispatchPlanner, &ctx, &policy()).unwrap();
    let focus = &jobs[0].worker_body["evidence_focus"]["records"];
    assert_eq!(focus[0]["payload_sha256"], "fresh-context");
    assert_eq!(focus.as_array().unwrap().len(), 1);
}

#[test]
fn builders_receive_an_explicit_one_based_build_round() {
    for remediation in 0..3 {
        let mut ctx = context();
        ctx.remediation_round = remediation;
        let jobs = schedule_effect("builder", Effect::DispatchBuilder, &ctx, &policy()).unwrap();
        assert_eq!(jobs[0].worker_body["build_round"], remediation + 1);
        assert!(
            jobs[0]
                .worker_projection_key
                .contains(&format!(":round:{}:", remediation + 1))
        );
    }
}

#[test]
fn role_evidence_focus_uses_exact_current_records_without_copying_history() {
    let mut ctx = context();
    ctx.immutable_evidence_bundle["records"] = json!({
        "events": [{"event_type":"ISSUE_AUTHORIZED","payload_sha256":"intake"}],
        "runs": [
            {"role":"planner","payload_sha256":"old-plan","payload":{"plan_version":0}},
            {"role":"planner","payload_sha256":"plan","payload":{"plan_version":1,"evidence":{"large_log":"x".repeat(100_000)}}},
            {"role":"builder","payload_sha256":"old-build","payload":{"plan_version":1,"head_sha":"c".repeat(40)}},
            {"role":"builder","payload_sha256":"build","payload":{"plan_version":1,"head_sha":"b".repeat(40)}},
            {"role":"reviewer-general","payload_sha256":"general","payload":{"reviewer_id":"general-sol","reviewed_head_sha":"b".repeat(40)}},
            {"role":"reviewer-secperf","payload_sha256":"kimi","payload":{"reviewer_id":"secperf-kimi","reviewed_head_sha":"b".repeat(40)}},
            {"role":"builder","payload_sha256":"different-head-build","payload":{"plan_version":1,"head_sha":"d".repeat(40)}}
        ],
        "findings": [
            {"origin_role":"general-sol","payload_sha256":"general-finding"},
            {"origin_role":"secperf-kimi","payload_sha256":"kimi-finding"}
        ],
        "evidence": [
            {"kind":"GITHUB_CI","payload_sha256":"old-ci","payload":{"pull_request":{"head_sha":"c".repeat(40)}}},
            {"kind":"GITHUB_CI","payload_sha256":"ci","payload":{"pull_request":{"head_sha":"b".repeat(40)}}},
            {"kind":"GITHUB_FINAL_PREFLIGHT","payload_sha256":"preflight","payload":{}},
            {"kind":"GITHUB_CI","payload_sha256":"different-head-ci","payload":{"pull_request":{"head_sha":"d".repeat(40)}}},
            {"kind":"GITHUB_REVIEW_FEEDBACK","payload_sha256":"feedback","payload":{"head_sha":"b".repeat(40)}}
        ]
    });
    for (effect, expected) in [
        (
            Effect::DispatchPlanner,
            vec!["intake", "plan", "general-finding", "kimi-finding"],
        ),
        (
            Effect::DispatchBuilder,
            vec![
                "intake",
                "plan",
                "build",
                "general",
                "kimi",
                "ci",
                "general-finding",
                "kimi-finding",
                "feedback",
            ],
        ),
        (
            Effect::DispatchReviewers,
            vec![
                "intake",
                "plan",
                "build",
                "general",
                "ci",
                "general-finding",
            ],
        ),
        (
            Effect::DispatchFinalReviewer,
            vec![
                "intake",
                "plan",
                "build",
                "general",
                "kimi",
                "ci",
                "general-finding",
                "kimi-finding",
                "preflight",
            ],
        ),
    ] {
        let jobs = schedule_effect("focus", effect, &ctx, &policy()).unwrap();
        let focus = &jobs[0].worker_body["evidence_focus"];
        assert_eq!(focus["schema_version"], 1);
        assert!(serde_json::to_vec(focus).unwrap().len() < 3000);
        let mut actual = focus["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|reference| {
                let record = ctx
                    .immutable_evidence_bundle
                    .pointer(reference["pointer"].as_str().unwrap())
                    .unwrap();
                assert_eq!(reference["payload_sha256"], record["payload_sha256"]);
                reference["payload_sha256"].as_str().unwrap()
            })
            .collect::<Vec<_>>();
        actual.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(actual, expected, "{effect:?}");
        if jobs.len() == 3 {
            let kimi = serde_json::to_string(&jobs[1].worker_body["evidence_focus"]).unwrap();
            assert!(kimi.contains("kimi-finding"));
            assert!(!kimi.contains("general"));
            let opus = serde_json::to_string(&jobs[2].worker_body["evidence_focus"]).unwrap();
            assert!(!opus.contains("finding"));
            assert!(!opus.contains("general"));
            assert!(!opus.contains("kimi"));
        }
        assert_eq!(
            jobs[0].worker_body["immutable_evidence_bundle"],
            ctx.immutable_evidence_bundle
        );
    }
}

#[test]
fn human_feedback_is_included_in_planner_evidence_focus() {
    let mut ctx = context();
    ctx.immutable_evidence_bundle["records"]["evidence"] = json!([
        {"kind":"HUMAN_DISCUSSION","payload_sha256":"human-feedback","payload":{"message":"Keep public API unchanged"}}
    ]);
    let planned =
        schedule_effect("feedback-plan", Effect::DispatchPlanner, &ctx, &policy()).unwrap();
    assert!(
        planned[0].worker_body["evidence_focus"]
            .to_string()
            .contains("human-feedback")
    );
}

#[test]
fn hermes_storage_is_projection_scoped_and_direct_tasks_are_unchanged() {
    let configured = policy()
        .with_hermes_scratch_root("/var/lib/pip/worktrees/hermes-scratch".into())
        .unwrap();
    let planner =
        schedule_effect("planner", Effect::DispatchPlanner, &context(), &configured).unwrap();
    let task = planner[0].hermes_task().unwrap();
    let storage = &task.body["storage"];
    assert_eq!(task.body["projection_key"], task.projection_key);
    let root = storage["root"].as_str().unwrap();
    assert!(root.starts_with("/var/lib/pip/worktrees/hermes-scratch/"));
    assert_eq!(root.rsplit('/').next().unwrap().len(), 16);
    let temporary = storage["temporary"].as_str().unwrap();
    // MDK binds through a private staging directory, not the final socket path.
    let staged = format!("{temporary}/.tmpabcdefgh/dev/.sock.4194304.wnd.sock/wnd.sock");
    assert!(staged.len() < 108, "Linux sockaddr_un overflow: {staged}");
    assert_eq!(storage["schema_version"], 3);
    assert_eq!(temporary, format!("{root}/t"));
    assert_eq!(storage["cargo_target"], format!("{root}/disposable/target"));
    assert_eq!(storage["results"], format!("{root}/results"));
    assert_eq!(
        task.body["immutable_evidence_ref"]["path"],
        format!("{root}/immutable-evidence.json")
    );
    assert_eq!(
        task.workspace,
        format!("dir:{}", storage["source"].as_str().unwrap())
    );
    let replay =
        schedule_effect("planner", Effect::DispatchPlanner, &context(), &configured).unwrap();
    assert_eq!(replay[0].hermes_task().unwrap(), task);
    assert!(
        policy()
            .with_hermes_scratch_root("/tmp/../escape".into())
            .is_err()
    );
    assert!(
        policy()
            .with_hermes_scratch_root("/var/lib/pip/worktrees/mdk/target".into())
            .is_err()
    );
}

fn context() -> DispatchContext {
    DispatchContext {
        case_id: CaseId::new(
            RepositoryId::new(NonZeroU64::new(984_321).unwrap()),
            IssueNumber::new(NonZeroU64::new(1240).unwrap()),
            WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
        ),
        state_revision: StateRevision::new(NonZeroU64::new(8).unwrap()),
        plan_version: Some(PlanVersion::new(NonZeroU32::new(1).unwrap())),
        remediation_round: 2,
        pr_number: Some(PullRequestNumber::new(NonZeroU64::new(77).unwrap())),
        head_sha: Some(GitSha::from_str(&"b".repeat(40)).unwrap()),
        skills_repository_commit: GitSha::from_str(&"a".repeat(40)).unwrap(),
        immutable_evidence_bundle: json!({"schema_version": 1, "sha256": "fixture"}),
    }
}

#[test]
fn hermes_attempt_limit_is_independent_of_direct_provider_retries() {
    let policy = policy()
        .with_max_provider_failures(3)
        .unwrap()
        .with_max_hermes_attempts(1)
        .unwrap();
    let task = schedule_effect("planner", Effect::DispatchPlanner, &context(), &policy).unwrap();
    assert_eq!(task[0].hermes_task().unwrap().max_retries, 1);
    assert!(policy.clone().with_max_hermes_attempts(0).is_err());
}

#[test]
fn planner_and_builder_use_ledger_authorized_workspaces_without_gate_dependencies() {
    let mut planner_context = context();
    planner_context.plan_version = None;
    planner_context.pr_number = None;
    planner_context.head_sha = None;
    let planner = schedule_effect(
        "effect-planner-1",
        Effect::DispatchPlanner,
        &planner_context,
        &policy(),
    )
    .unwrap();
    assert_eq!(planner.len(), 1);
    assert_eq!(planner[0].role, WorkerRole::Planner);
    assert_eq!(planner[0].execution(), ExecutionKind::Hermes);
    assert!(matches!(
        planner[0].direct_task(),
        Err(DispatchError::WrongExecutor)
    ));
    let worker = planner[0].hermes_task().unwrap();
    assert!(worker.parent_task_ids.is_empty());
    assert_eq!(worker.assignee, "planner");
    assert_eq!(
        worker.workspace,
        "dir:/var/lib/pip/worktrees/mdk/repo-984321-issue-1240-workflow-1"
    );
    assert_eq!(worker.model, "gpt-5.6-sol");
    assert_eq!(worker.body["state_revision"], 8);
    assert_eq!(worker.body["plan_version"], 1);
    assert_eq!(worker.body["requested_model"], "openai-codex/gpt-5.6-sol");
    assert_eq!(worker.body["skills_repository_commit"], "a".repeat(40));

    let builder = schedule_effect(
        "effect-builder-r2",
        Effect::DispatchBuilder,
        &context(),
        &policy(),
    )
    .unwrap();
    assert_eq!(builder[0].role, WorkerRole::Builder);
    assert_eq!(builder[0].execution(), ExecutionKind::Direct);
    assert!(matches!(
        builder[0].hermes_task(),
        Err(DispatchError::WrongExecutor)
    ));
    let direct = builder[0].direct_task().unwrap();
    assert_eq!(direct.source_effect_id, "effect-builder-r2");
    assert_eq!(direct.role, WorkerRole::Builder);
    assert_eq!(direct.task_id, builder[0].worker_projection_key);
    assert_eq!(direct.provider, "cursor");
    assert_eq!(direct.model, "cursor-grok-4.6-high-fast");
    assert_eq!(
        direct.workspace,
        "/var/lib/pip/worktrees/mdk/repo-984321-issue-1240-workflow-1"
    );
    assert_eq!(builder[0].worker_body["remediation_round"], 2);
    assert_eq!(
        builder[0].worker_body["assigned_branch"],
        "pip/repo-984321/issue-1240/workflow-1"
    );
    assert_eq!(
        builder[0].worker_body["assigned_worktree"],
        "/var/lib/pip/worktrees/mdk/repo-984321-issue-1240-workflow-1"
    );
    assert_eq!(
        builder[0].worker_body["requested_model"],
        "cursor/cursor-grok-4.6-high-fast"
    );
    assert!(builder[0].worker_projection_key.contains("round:3"));
}

#[test]
fn review_effect_expands_policy_defined_instances_against_one_exact_head() {
    let dispatches = schedule_effect(
        "effect-reviews-r3",
        Effect::DispatchReviewers,
        &context(),
        &policy(),
    )
    .unwrap();
    assert_eq!(dispatches.len(), 3);
    assert_eq!(dispatches[0].role, WorkerRole::ReviewerGeneral);
    assert_eq!(dispatches[1].role, WorkerRole::ReviewerSecperf);
    assert_eq!(dispatches[0].worker_body["reviewer_id"], "general-sol");
    assert_eq!(dispatches[0].worker_body["review_mode"], "required");
    assert_eq!(dispatches[1].worker_body["reviewer_id"], "secperf-kimi");
    assert_eq!(dispatches[1].worker_body["review_mode"], "required");
    assert_eq!(dispatches[2].worker_body["reviewer_id"], "secperf-opus");
    assert_eq!(dispatches[2].worker_body["review_mode"], "shadow");
    assert!(
        dispatches
            .iter()
            .map(|dispatch| &dispatch.worker_projection_key)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == 3
    );
    for dispatch in dispatches {
        assert_eq!(dispatch.worker_body["pr_number"], 77);
        assert_eq!(dispatch.worker_body["expected_head_sha"], "b".repeat(40));
        assert_eq!(dispatch.worker_body["review_round"], 3);
        match dispatch.execution() {
            ExecutionKind::Hermes => {
                assert!(dispatch.hermes_task().unwrap().parent_task_ids.is_empty());
            }
            ExecutionKind::Direct => {
                let direct = dispatch.direct_task().unwrap();
                assert_eq!(direct.role, WorkerRole::ReviewerSecperf);
                assert!(direct.task_id.contains("secperf-"));
            }
        }
    }
}

#[test]
fn isolated_reviews_bind_distinct_snapshots_without_changing_builder_workspace() {
    let policy = policy().with_isolated_reviews().with_cargo_jobs(2).unwrap();
    let reviews =
        schedule_effect("reviews", Effect::DispatchReviewers, &context(), &policy).unwrap();
    let mut roots = std::collections::BTreeSet::new();
    for review in reviews {
        assert_eq!(review.worker_body["cargo_jobs"], 2);
        let snapshot = &review.worker_body["review_snapshot"];
        assert_eq!(snapshot["head_sha"], "b".repeat(40));
        assert!(snapshot["source"].as_str().unwrap().contains("repo-"));
        let root = snapshot["root"].as_str().unwrap();
        assert!(root.contains("/.reviews/"));
        roots.insert(root.to_owned());
        let expected = format!("{root}/source");
        match review.execution() {
            ExecutionKind::Hermes => assert_eq!(
                review.hermes_task().unwrap().workspace,
                format!("dir:{expected}")
            ),
            ExecutionKind::Direct => assert_eq!(review.direct_task().unwrap().workspace, expected),
        }
    }
    assert_eq!(roots.len(), 3);
    let builder = schedule_effect("builder", Effect::DispatchBuilder, &context(), &policy).unwrap();
    assert!(builder[0].worker_body.get("review_snapshot").is_none());
}

#[test]
fn remediation_is_dynamic_and_final_review_requires_exact_head() {
    let mut later = context();
    later.remediation_round = 7;
    later.state_revision = StateRevision::new(NonZeroU64::new(42).unwrap());
    let builder = schedule_effect(
        "effect-builder-r7",
        Effect::DispatchBuilder,
        &later,
        &policy(),
    )
    .unwrap();
    assert_eq!(builder[0].worker_body["remediation_round"], 7);
    assert!(builder[0].worker_projection_key.contains("round:8"));

    let final_review = schedule_effect(
        "effect-final-r7",
        Effect::DispatchFinalReviewer,
        &later,
        &policy(),
    )
    .unwrap();
    assert_eq!(final_review[0].role, WorkerRole::FinalReviewer);
    assert_eq!(
        final_review[0].worker_body["expected_head_sha"],
        "b".repeat(40)
    );

    later.head_sha = None;
    assert!(matches!(
        schedule_effect(
            "effect-final-r7",
            Effect::DispatchFinalReviewer,
            &later,
            &policy(),
        ),
        Err(DispatchError::MissingExactHead)
    ));
}

#[test]
fn role_policy_rejects_duplicates_missing_skills_and_model_fallbacks() {
    assert!(matches!(
        WorkflowPolicy::new(
            "pip-mdk",
            "relative/workspaces/mdk",
            "pip/",
            policy().roles().to_vec()
        ),
        Err(DispatchError::InvalidPolicy)
    ));

    let mut roles = policy().roles().to_vec();
    roles.push(roles[0].clone());
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "/var/lib/pip/worktrees/mdk", "pip/", roles),
        Err(DispatchError::InvalidPolicy)
    ));

    let mut roles = policy().roles().to_vec();
    roles[4].reviewer_id = Some("secperf-kimi".into());
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "/var/lib/pip/worktrees/mdk", "pip/", roles),
        Err(DispatchError::InvalidPolicy)
    ));

    let roles = policy()
        .roles()
        .iter()
        .filter(|role| {
            role.role != WorkerRole::ReviewerGeneral
                || role.review_mode != Some(ReviewMode::Required)
        })
        .cloned()
        .collect();
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "/var/lib/pip/worktrees/mdk", "pip/", roles),
        Err(DispatchError::InvalidPolicy)
    ));

    let mut roles = policy().roles().to_vec();
    roles[1].skills = vec!["builder-grok".into()];
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "/var/lib/pip/worktrees/mdk", "pip/", roles),
        Err(DispatchError::InvalidPolicy)
    ));

    let mut roles = policy().roles().to_vec();
    roles[1].model = "auto".into();
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "/var/lib/pip/worktrees/mdk", "pip/", roles),
        Err(DispatchError::InvalidPolicy)
    ));
}

#[test]
fn claimed_outbox_dispatch_is_bound_to_the_current_case_revision() {
    let case = StoredCase {
        case_key: "repo:984321#1240@1".into(),
        repository_id: 984_321,
        issue_number: 1240,
        workflow_version: 1,
        state: "REVIEWING".into(),
        state_revision: 8,
        policy_revision: 1,
        remediation_round: 2,
        plan_version: 1,
        pr_number: Some(77),
        head_sha: Some("b".repeat(40)),
    };
    let effect = ClaimedEffect {
        effect_id: "effect-reviewers-1".into(),
        case_key: case.case_key.clone(),
        state_revision: 8,
        effect_type: "DISPATCH_REVIEWERS".into(),
        payload: json!({"case_key": case.case_key}),
        lease_owner: "controller-1".into(),
        lease_until: 200,
    };

    let skills_commit = GitSha::from_str(&"a".repeat(40)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut store = pip_store::Store::open(directory.path().join("ledger.db")).unwrap();
    store
        .record_policy(&pip_store::PolicyInput {
            repository_id: case.repository_id,
            revision: case.policy_revision,
            accepted_at: 1,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&pip_store::NewCase {
            case_key: case.case_key.clone(),
            repository_id: case.repository_id,
            issue_number: case.issue_number,
            workflow_version: case.workflow_version,
            policy_revision: case.policy_revision,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: pip_store::EventInput {
                event_id: "event-fixture".into(),
                event_type: "FIXTURE".into(),
                payload: json!({"fixture": true}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &pip_store::TransitionInput {
                case_key: case.case_key.clone(),
                expected_revision: 1,
                next_state: "REVIEWING".into(),
                remediation_round: case.remediation_round,
                plan_version: case.plan_version,
                pr_number: case.pr_number,
                head_sha: case.head_sha.clone(),
                observed_at: 2,
                event: pip_store::EventInput {
                    event_id: "event-reviewing-fixture".into(),
                    event_type: "CI_ACCEPTED".into(),
                    payload: json!({"fixture": true}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![pip_store::EffectInput {
                    effect_id: effect.effect_id.clone(),
                    effect_type: effect.effect_type.clone(),
                    payload: effect.payload.clone(),
                }],
            },
            None,
        )
        .unwrap();
    let stored_case = store.case(&case.case_key).unwrap().unwrap();
    let claimed = store
        .claim_effect_matching("controller-1", 3, 30, &["DISPATCH_REVIEWERS"])
        .unwrap()
        .unwrap();
    let dispatches =
        schedule_claimed_dispatch(&claimed, &stored_case, &store, &policy(), skills_commit)
            .unwrap();
    assert_eq!(dispatches.len(), 3);
    assert_eq!(dispatches[0].role, WorkerRole::ReviewerGeneral);
    assert_eq!(dispatches[1].role, WorkerRole::ReviewerSecperf);
    assert_eq!(dispatches[2].role, WorkerRole::ReviewerSecperf);

    let mut invented_case = stored_case.clone();
    invented_case.state_revision = 3;
    let mut invented_claim = claimed.clone();
    invented_claim.state_revision = 3;
    assert_eq!(
        schedule_claimed_dispatch(
            &invented_claim,
            &invented_case,
            &store,
            &policy(),
            skills_commit,
        ),
        Err(DispatchError::InvalidEvidenceBundle)
    );

    let mut stale = claimed;
    stale.state_revision = 1;
    assert_eq!(
        schedule_claimed_dispatch(&stale, &stored_case, &store, &policy(), skills_commit),
        Err(DispatchError::StaleEffect)
    );
}

#[test]
fn oversized_immutable_history_fails_before_worker_projection() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = pip_store::Store::open(directory.path().join("ledger.db")).unwrap();
    let case_key = "repo:984321#1240@1";
    store
        .record_policy(&pip_store::PolicyInput {
            repository_id: 984_321,
            revision: 1,
            accepted_at: 1,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&pip_store::NewCase {
            case_key: case_key.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: pip_store::EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"oversized": "x".repeat(600 * 1024)}),
            },
            effects: vec![pip_store::EffectInput {
                effect_id: "effect-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key": case_key}),
            }],
        })
        .unwrap();
    let claimed = store
        .claim_effect_matching("controller-1", 2, 30, &["DISPATCH_PLANNER"])
        .unwrap()
        .unwrap();
    let case = store.case(case_key).unwrap().unwrap();

    assert_eq!(
        schedule_claimed_dispatch(
            &claimed,
            &case,
            &store,
            &policy(),
            GitSha::from_str(&"a".repeat(40)).unwrap(),
        ),
        Err(DispatchError::InvalidEvidenceBundle)
    );
}

#[test]
fn final_reviewer_receives_the_committed_preflight_and_complete_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = pip_store::Store::open(directory.path().join("ledger.db")).unwrap();
    let case_key = "repo:984321#1240@1";
    store
        .record_policy(&pip_store::PolicyInput {
            repository_id: 984_321,
            revision: 1,
            accepted_at: 1,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&pip_store::NewCase {
            case_key: case_key.into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: pip_store::EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"issue": {"title": "fix it"}}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    let mut plan = serde_json::from_str::<serde_json::Value>(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()["results"][0]
        .clone();
    plan["evidence"]["diagnostic_rows"] = json!(vec!["x".repeat(300); 1000]);
    store
        .apply_transition(
            &pip_store::TransitionInput {
                case_key: case_key.into(),
                expected_revision: 1,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 2,
                event: pip_store::EventInput {
                    event_id: "accepted-plan".into(),
                    event_type: "PLAN_RECORDED".into(),
                    payload: plan.clone(),
                },
                run: Some(pip_store::RunInput {
                    run_id: "run-plan".into(),
                    task_id: "task-plan".into(),
                    role: "planner".into(),
                    payload: plan.clone(),
                }),
                evidence: vec![],
                findings: vec![],
                effects: vec![],
            },
            None,
        )
        .unwrap();
    store
        .apply_transition(
            &pip_store::TransitionInput {
                case_key: case_key.into(),
                expected_revision: 2,
                next_state: "FINAL_REVIEW".into(),
                remediation_round: 2,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 2,
                event: pip_store::EventInput {
                    event_id: "event-final-preflight".into(),
                    event_type: "FINAL_PREFLIGHT_ACCEPTED".into(),
                    payload: json!({"verdict": "ACCEPTED", "head_sha": "b".repeat(40)}),
                },
                run: None,
                evidence: vec![pip_store::EvidenceInput {
                    evidence_id: "evidence-final-preflight".into(),
                    kind: "GITHUB_FINAL_PREFLIGHT".into(),
                    source: "github-pr-77".into(),
                    payload: json!({
                        "pull_request": {"number": 77, "head_sha": "b".repeat(40)},
                        "review_threads": [],
                    }),
                }],
                findings: Vec::new(),
                effects: vec![pip_store::EffectInput {
                    effect_id: "effect-final-review".into(),
                    effect_type: "DISPATCH_FINAL_REVIEWER".into(),
                    payload: json!({"case_key": case_key}),
                }],
            },
            None,
        )
        .unwrap();
    let claimed = store
        .claim_effect_matching("controller-1", 3, 30, &["DISPATCH_FINAL_REVIEWER"])
        .unwrap()
        .unwrap();
    let case = store.case(case_key).unwrap().unwrap();

    let dispatch = schedule_claimed_dispatch(
        &claimed,
        &case,
        &store,
        &policy(),
        GitSha::from_str(&"a".repeat(40)).unwrap(),
    )
    .unwrap()
    .remove(0);

    assert_eq!(dispatch.role, WorkerRole::FinalReviewer);
    let bundle = &dispatch.worker_body["immutable_evidence_bundle"];
    assert_eq!(bundle["schema_version"], 2);
    assert_eq!(bundle["bound_state_revision"], 3);
    assert_eq!(bundle["records"]["events"].as_array().unwrap().len(), 3);
    let event = &bundle["records"]["events"][1];
    let run = &bundle["records"]["runs"][0];
    assert!(event.get("payload").is_none());
    assert_eq!(
        event["payload_ref"],
        json!({"run_id":"run-plan","payload_sha256":run["payload_sha256"]})
    );
    assert_eq!(run["payload"], plan);
    assert_eq!(event["payload_sha256"], run["payload_sha256"]);
    assert!(serde_json::to_vec(bundle).unwrap().len() < 350_000);
    // Export compaction never rewrites accepted ledger records.
    assert_eq!(
        store.immutable_history_for_case(case_key).unwrap().events[1].payload,
        plan
    );
    assert_eq!(
        bundle["records"]["evidence"][0]["kind"],
        "GITHUB_FINAL_PREFLIGHT"
    );
    assert_eq!(
        bundle["records"]["evidence"][0]["payload"]["pull_request"]["head_sha"],
        "b".repeat(40)
    );
}
