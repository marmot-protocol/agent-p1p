use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::WorkerRole;
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
        profile: profile.into(),
        execution,
        provider: provider.into(),
        model: model.into(),
        max_runtime: max_runtime.into(),
        priority: 10,
        skills: vec!["workflow-contract".into(), profile.into()],
    }
}

fn policy() -> WorkflowPolicy {
    WorkflowPolicy::new(
        "pip-mdk",
        "scratch",
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
                "composer-2.5",
                "60m",
            ),
            role(
                WorkerRole::ReviewerGeneral,
                "reviewer-general",
                ExecutionKind::Hermes,
                "openai-codex",
                "gpt-5.6-sol",
                "30m",
            ),
            role(
                WorkerRole::ReviewerSecperf,
                "reviewer-secperf",
                ExecutionKind::Direct,
                "cursor",
                "claude-opus-4-8-thinking-high",
                "30m",
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
fn planner_and_builder_dispatches_are_blocked_behind_controller_gates() {
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
    assert!(planner[0].gate.body["case_key"].is_string());
    let worker = planner[0].bind_gate("gate-1").unwrap();
    assert_eq!(worker.parent_task_ids, ["gate-1"]);
    assert_eq!(worker.assignee, "planner");
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
    assert_eq!(builder[0].worker_body["remediation_round"], 2);
    assert_eq!(
        builder[0].worker_body["requested_model"],
        "cursor/composer-2.5"
    );
    assert!(builder[0].worker_projection_key.contains("round:2"));
}

#[test]
fn review_effect_expands_to_two_independent_same_head_dispatches() {
    let dispatches = schedule_effect(
        "effect-reviews-r3",
        Effect::DispatchReviewers,
        &context(),
        &policy(),
    )
    .unwrap();
    assert_eq!(dispatches.len(), 2);
    assert_eq!(dispatches[0].role, WorkerRole::ReviewerGeneral);
    assert_eq!(dispatches[1].role, WorkerRole::ReviewerSecperf);
    assert_ne!(dispatches[0].gate.effect_id, dispatches[1].gate.effect_id);
    for dispatch in dispatches {
        assert_eq!(dispatch.worker_body["pr_number"], 77);
        assert_eq!(dispatch.worker_body["expected_head_sha"], "b".repeat(40));
        assert_eq!(dispatch.worker_body["review_round"], 3);
        assert!(dispatch.bind_gate("gate-1").unwrap().parent_task_ids == ["gate-1"]);
    }
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
    assert!(builder[0].worker_projection_key.contains("round:7"));

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
    let mut roles = policy().roles().to_vec();
    roles.push(roles[0].clone());
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "scratch", roles),
        Err(DispatchError::InvalidPolicy)
    ));

    let mut roles = policy().roles().to_vec();
    roles[1].skills = vec!["builder-grok".into()];
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "scratch", roles),
        Err(DispatchError::InvalidPolicy)
    ));

    let mut roles = policy().roles().to_vec();
    roles[1].model = "auto".into();
    assert!(matches!(
        WorkflowPolicy::new("pip-mdk", "scratch", roles),
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
    assert_eq!(dispatches.len(), 2);
    assert_eq!(dispatches[0].role, WorkerRole::ReviewerGeneral);
    assert_eq!(dispatches[1].role, WorkerRole::ReviewerSecperf);

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
    store
        .apply_transition(
            &pip_store::TransitionInput {
                case_key: case_key.into(),
                expected_revision: 1,
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
    assert_eq!(bundle["bound_state_revision"], 2);
    assert_eq!(bundle["records"]["events"].as_array().unwrap().len(), 2);
    assert_eq!(
        bundle["records"]["evidence"][0]["kind"],
        "GITHUB_FINAL_PREFLIGHT"
    );
    assert_eq!(
        bundle["records"]["evidence"][0]["payload"]["pull_request"]["head_sha"],
        "b".repeat(40)
    );
}
