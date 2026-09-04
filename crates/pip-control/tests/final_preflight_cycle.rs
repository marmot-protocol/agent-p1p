use std::collections::BTreeSet;

use pip_contracts::{ReviewMode, WorkerBinding, WorkerResult};
use pip_control::{
    CiCycle, FinalPreflightCycle, FinalPreflightSource, IntakeSource, PullRequestSource,
    load_repository_policy, reconcile_ci_once, reconcile_final_preflight_once,
};
use pip_controller::ingest_worker_result;
use pip_github::{
    CheckConclusion, CheckRunSnapshot, CheckStatus, CommitStatusState, GitHubError, IntakeSnapshot,
    IssueCommentSnapshot, IssueContentSnapshot, IssueSnapshot, LabelEvent, PullRequestEvidence,
    PullRequestSnapshot, RepositorySnapshot, ReviewSnapshot, ReviewState, ReviewThreadSnapshot,
};
use pip_store::{EffectInput, EventInput, NewCase, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Clone)]
struct FixtureSource {
    evidence: PullRequestEvidence,
    threads: Vec<ReviewThreadSnapshot>,
    issue_authorized: bool,
}

impl PullRequestSource for FixtureSource {
    fn pull_request(
        &self,
        _owner: &str,
        _repository: &str,
        _repository_id: u64,
        _pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError> {
        Ok(self.evidence.clone())
    }
}

impl IntakeSource for FixtureSource {
    fn discover(
        &self,
        _owner: &str,
        _repository: &str,
        _label: &str,
    ) -> Result<Vec<IssueSnapshot>, GitHubError> {
        Ok(vec![issue_snapshot(self.issue_authorized)])
    }

    fn intake(
        &self,
        _owner: &str,
        _repository: &str,
        issue_number: u64,
    ) -> Result<IntakeSnapshot, GitHubError> {
        assert_eq!(issue_number, 1240);
        Ok(IntakeSnapshot {
            repository: RepositorySnapshot {
                id: 984_321,
                full_name: "marmot-protocol/mdk".into(),
                default_branch: "master".into(),
            },
            issue: issue_snapshot(self.issue_authorized),
            issue_content: IssueContentSnapshot {
                author_id: 1000,
                title: "Fix issue #1240".into(),
                body: "Original issue body.".into(),
                created_at: "2026-08-19T00:00:00Z".into(),
                updated_at: "2026-08-20T00:00:00Z".into(),
            },
            label_events: vec![LabelEvent {
                id: 91,
                labeled: true,
                actor_id: 202_880,
                label: "pip-ok".into(),
                created_at: "2026-08-20T00:00:00Z".into(),
            }],
            comments: vec![IssueCommentSnapshot {
                id: 10001,
                actor_id: 1000,
                issue_number: 1240,
                html_url: "https://github.test/marmot-protocol/mdk/issues/1240#issuecomment-10001"
                    .into(),
                body: "Use the exact accepted plan.".into(),
                body_sha256: "fixture-digest".into(),
                created_at: "2026-08-20T00:01:00Z".into(),
                updated_at: "2026-08-20T00:01:00Z".into(),
            }],
        })
    }
}

fn issue_snapshot(authorized: bool) -> IssueSnapshot {
    IssueSnapshot {
        id: 555,
        number: 1240,
        open: authorized,
        is_pull_request: false,
        labels: BTreeSet::from(["pip-ok".into()]),
    }
}

impl FinalPreflightSource for FixtureSource {
    fn review_threads(
        &self,
        _owner: &str,
        _repository: &str,
        _repository_id: u64,
        _pull_request_number: u64,
    ) -> Result<Vec<ReviewThreadSnapshot>, GitHubError> {
        Ok(self.threads.clone())
    }
}

#[test]
fn exact_published_reviews_clean_ci_and_resolved_threads_release_final_review() {
    let directory = tempfile::tempdir().unwrap();
    let policy = active_policy();
    let source = accepted_source();
    let mut store = final_review_store(directory.path().join("ledger.db"), &policy, &source);

    let result = reconcile_final_preflight_once(
        &source,
        &policy,
        &mut store,
        200,
        "final-preflight",
        30,
        true,
    )
    .unwrap();

    assert_eq!(
        result,
        FinalPreflightCycle::Accepted {
            case_key: "repo:984321#1240@1".into(),
            head_sha: "b".repeat(40),
        }
    );
    assert_eq!(store.evidence_count().unwrap(), 6);
    let history = store
        .immutable_history_for_case("repo:984321#1240@1")
        .unwrap();
    let preflight = history
        .evidence
        .iter()
        .find(|evidence| evidence.kind == "GITHUB_FINAL_PREFLIGHT")
        .unwrap();
    assert_eq!(
        preflight.payload["issue_authorization"]["issue_content"]["title"],
        "Fix issue #1240"
    );
    assert_eq!(
        preflight.payload["issue_authorization"]["comments"][0]["body"],
        "Use the exact accepted plan."
    );
    let effect = store
        .claim_effect_matching("dispatcher", 200, 30, &["DISPATCH_FINAL_REVIEWER"])
        .unwrap()
        .unwrap();
    assert_eq!(effect.state_revision, 11);
}

#[test]
fn every_policy_required_reviewer_instance_must_approve_the_exact_head() {
    let directory = tempfile::tempdir().unwrap();
    let mut value = serde_json::to_value(active_policy()).unwrap();
    value["roles"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|role| role["reviewer_id"] == "secperf-opus")
        .unwrap()["review_mode"] = json!("required");
    let policy = load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap();
    let source = accepted_source();
    let mut store = final_review_store(directory.path().join("ledger.db"), &policy, &source);

    let result = reconcile_final_preflight_once(
        &source,
        &policy,
        &mut store,
        200,
        "final-preflight",
        30,
        true,
    )
    .unwrap();
    assert!(matches!(
        result,
        FinalPreflightCycle::Pending { blockers, .. }
            if blockers.contains(&"MISSING_LEDGER_APPROVAL:secperf-opus".into())
    ));
}

#[test]
fn unresolved_threads_fail_closed_without_stranding_the_observation_lease() {
    let directory = tempfile::tempdir().unwrap();
    let policy = active_policy();
    let mut source = accepted_source();
    source.threads[0].is_resolved = false;
    let mut store = final_review_store(directory.path().join("ledger.db"), &policy, &source);

    let result = reconcile_final_preflight_once(
        &source,
        &policy,
        &mut store,
        200,
        "final-preflight",
        30,
        true,
    )
    .unwrap();
    assert!(matches!(
        result,
        FinalPreflightCycle::Pending { blockers, .. }
            if blockers == ["UNRESOLVED_REVIEW_THREAD:PRRT_1"]
    ));
    assert_eq!(store.evidence_count().unwrap(), 5);
    assert!(
        store
            .claim_effect_matching("retry", 200, 30, &["OBSERVE_FINAL_PREFLIGHT"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn dirty_mergeability_and_missing_or_stale_role_reviews_cannot_open_the_gate() {
    let directory = tempfile::tempdir().unwrap();
    let policy = active_policy();
    let mut source = accepted_source();
    source.evidence.pull_request.mergeable_state = "dirty".into();
    source.evidence.reviews[1].commit_id = Some("a".repeat(40));
    source.evidence.reviews[1].exact_head = false;
    let mut store = final_review_store(directory.path().join("ledger.db"), &policy, &source);

    let result = reconcile_final_preflight_once(
        &source,
        &policy,
        &mut store,
        200,
        "final-preflight",
        30,
        true,
    )
    .unwrap();
    assert!(matches!(
        result,
        FinalPreflightCycle::Pending { blockers, .. }
            if blockers.contains(&"PR_NOT_CLEANLY_MERGEABLE".into())
                && blockers.contains(&"MISSING_EXACT_HEAD_APPROVAL:reviewer-secperf".into())
    ));
    assert!(
        store
            .claim_effect_matching("dispatcher", 200, 30, &["DISPATCH_FINAL_REVIEWER"])
            .unwrap()
            .is_none()
    );
}

#[test]
fn stale_authorization_never_claims_or_accepts_final_preflight() {
    let directory = tempfile::tempdir().unwrap();
    let policy = active_policy();
    let source = accepted_source();
    let mut store = final_review_store(directory.path().join("ledger.db"), &policy, &source);

    assert_eq!(
        reconcile_final_preflight_once(
            &source,
            &policy,
            &mut store,
            200,
            "final-preflight",
            30,
            false,
        )
        .unwrap(),
        FinalPreflightCycle::AuthorizationBlocked
    );
    assert!(
        store
            .claim_effect_matching("retry", 200, 30, &["OBSERVE_FINAL_PREFLIGHT"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn final_preflight_refetches_issue_authorization_and_fails_closed_on_drift() {
    let directory = tempfile::tempdir().unwrap();
    let policy = active_policy();
    let mut source = accepted_source();
    source.issue_authorized = false;
    let mut store = final_review_store(directory.path().join("ledger.db"), &policy, &source);

    assert_eq!(
        reconcile_final_preflight_once(
            &source,
            &policy,
            &mut store,
            200,
            "final-preflight",
            30,
            true,
        )
        .unwrap(),
        FinalPreflightCycle::AuthorizationBlocked
    );
    assert_eq!(store.evidence_count().unwrap(), 5);
    assert!(
        store
            .claim_effect_matching("retry", 200, 30, &["OBSERVE_FINAL_PREFLIGHT"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn historical_blocker_requires_exact_head_resolution_and_origin_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let policy = active_policy();
    let mut source = accepted_source();
    let mut store =
        remediated_final_review_store(directory.path().join("ledger.db"), &policy, &mut source);

    let result = reconcile_final_preflight_once(
        &source,
        &policy,
        &mut store,
        300,
        "final-preflight",
        30,
        true,
    )
    .unwrap();
    assert!(matches!(
        result,
        FinalPreflightCycle::Pending { blockers, .. }
            if blockers == ["MISSING_ORIGIN_CONFIRMATION:GENERAL-R1-001"]
    ));
}

fn final_review_store(
    path: std::path::PathBuf,
    policy: &pip_control::RepositoryPolicy,
    source: &FixtureSource,
) -> Store {
    let mut store = planning_store(path);
    let results = results();
    ingest_worker_result(
        &mut store,
        &policy.case_policy(),
        &binding(&results[0]),
        &results[0],
    )
    .unwrap();
    accept_plan(&mut store, &results[0]);
    ingest_worker_result(
        &mut store,
        &policy.case_policy(),
        &binding(&results[1]),
        &results[1],
    )
    .unwrap();
    publish_build(&mut store, &results[1]);
    assert!(matches!(
        reconcile_ci_once(source, policy, &mut store, 100).unwrap(),
        CiCycle::Transitioned { .. }
    ));
    for result in &results[2..4] {
        ingest_worker_result(&mut store, &policy.case_policy(), &binding(result), result).unwrap();
    }
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "FINAL_REVIEW"
    );
    mark_reviews_published(&mut store);
    store
}

fn planning_store(path: std::path::PathBuf) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: active_policy().revision,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label":"pip-ok"}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key":"repo:984321#1240@1"}),
            }],
        })
        .unwrap();
    store
}

fn remediated_final_review_store(
    path: std::path::PathBuf,
    policy: &pip_control::RepositoryPolicy,
    source: &mut FixtureSource,
) -> Store {
    let mut store = planning_store(path);
    let mut results = results();
    ingest_worker_result(
        &mut store,
        &policy.case_policy(),
        &binding(&results[0]),
        &results[0],
    )
    .unwrap();
    accept_plan(&mut store, &results[0]);
    ingest_worker_result(
        &mut store,
        &policy.case_policy(),
        &binding(&results[1]),
        &results[1],
    )
    .unwrap();
    publish_build(&mut store, &results[1]);
    reconcile_ci_once(source, policy, &mut store, 100).unwrap();

    let mut changed = serde_json::to_value(&results[2]).unwrap();
    changed["outcome"] = json!("REQUEST_CHANGES");
    changed["blocking_findings"] = json!([{
        "id": "GENERAL-R1-001",
        "summary": "unsafe edge",
        "defect": "unchecked edge",
        "consequence": "wrong state",
        "corrective_direction": "validate it",
        "required_evidence": ["regression test"]
    }]);
    results[2] = serde_json::from_value(changed).unwrap();
    for result in &results[2..4] {
        ingest_worker_result(&mut store, &policy.case_policy(), &binding(result), result).unwrap();
    }

    let mut builder = serde_json::to_value(&results[1]).unwrap();
    builder["task_id"] = json!("builder-2");
    builder["build_round"] = json!(2);
    builder["head_sha"] = json!("c".repeat(40));
    builder["finding_resolutions"] = json!([{
        "finding_id": "GENERAL-R1-001",
        "resolution_commit": "c".repeat(40),
        "resolved_head_sha": "c".repeat(40),
        "resolution_summary": "validated edge",
        "tests": ["edge regression"]
    }]);
    let builder: WorkerResult = serde_json::from_value(builder).unwrap();
    ingest_worker_result(
        &mut store,
        &policy.case_policy(),
        &binding(&builder),
        &builder,
    )
    .unwrap();
    publish_build(&mut store, &builder);

    source.evidence.pull_request.head_sha = "c".repeat(40);
    source.evidence.check_runs[0].head_sha = "c".repeat(40);
    for review in &mut source.evidence.reviews {
        review.commit_id = Some("c".repeat(40));
    }
    reconcile_ci_once(source, policy, &mut store, 200).unwrap();

    for (index, original) in results[2..4].iter().enumerate() {
        let mut review = serde_json::to_value(original).unwrap();
        review["task_id"] = json!(format!("review-{}-2", index + 1));
        review["outcome"] = json!("APPROVE");
        review["review_round"] = json!(2);
        review["reviewed_head_sha"] = json!("c".repeat(40));
        review["blocking_findings"] = json!([]);
        review["finding_confirmations"] = json!([]);
        let review: WorkerResult = serde_json::from_value(review).unwrap();
        ingest_worker_result(
            &mut store,
            &policy.case_policy(),
            &binding(&review),
            &review,
        )
        .unwrap();
    }
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "FINAL_REVIEW"
    );
    mark_reviews_published(&mut store);
    store
}

fn accept_plan(store: &mut Store, result: &WorkerResult) {
    let WorkerResult::Planner(plan) = result else {
        panic!("planner result required");
    };
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: case.case_key,
                expected_revision: case.state_revision,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 0,
                plan_version: plan.plan_version,
                pr_number: None,
                head_sha: None,
                observed_at: 3,
                event: EventInput {
                    event_id: "event-plan-published".into(),
                    event_type: "PROCEED".into(),
                    payload: json!({"planner_result": plan}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-builder".into(),
                    effect_type: "DISPATCH_BUILDER".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
}

fn publish_build(store: &mut Store, result: &WorkerResult) {
    let WorkerResult::Builder(build) = result else {
        panic!("builder result required");
    };
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: case.case_key,
                expected_revision: case.state_revision,
                next_state: "WAITING_CI".into(),
                remediation_round: case.remediation_round,
                plan_version: case.plan_version,
                pr_number: Some(77),
                head_sha: build.head_sha.clone(),
                observed_at: 4 + u64::from(build.build_round),
                event: EventInput {
                    event_id: format!("event-draft-pr-published-{}", build.build_round),
                    event_type: "REVIEW_READY".into(),
                    payload: json!({"builder_result": build}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: format!("effect-ci-{}", build.build_round),
                    effect_type: "OBSERVE_CI".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
}

fn mark_reviews_published(store: &mut Store) {
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    store
        .apply_transition(
            &pip_store::TransitionInput {
                case_key: case.case_key,
                expected_revision: case.state_revision,
                next_state: "FINAL_REVIEW".into(),
                remediation_round: case.remediation_round,
                plan_version: case.plan_version,
                pr_number: case.pr_number,
                head_sha: case.head_sha,
                observed_at: 250,
                event: EventInput {
                    event_id: format!("event-reviews-published-{}", case.state_revision),
                    event_type: "REVIEWS_PUBLISHED".into(),
                    payload: json!({"fixture":true}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: format!("effect-final-preflight-{}", case.state_revision),
                    effect_type: "OBSERVE_FINAL_PREFLIGHT".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
}

fn accepted_source() -> FixtureSource {
    FixtureSource {
        evidence: PullRequestEvidence {
            pull_request: PullRequestSnapshot {
                id: 9,
                number: 77,
                open: true,
                draft: true,
                merged: false,
                merge_commit_sha: None,
                mergeable: Some(true),
                mergeable_state: "clean".into(),
                author_id: 202_880,
                head_repository_id: 984_321,
                head_repository: "marmot-protocol/mdk".into(),
                head_branch: "pip/repo-984321/issue-1240/workflow-1".into(),
                head_sha: "b".repeat(40),
                base_branch: "master".into(),
                base_sha: "a".repeat(40),
            },
            check_runs: vec![CheckRunSnapshot {
                id: 1,
                app_id: 1,
                name: "test".into(),
                head_sha: "b".repeat(40),
                status: CheckStatus::Completed,
                conclusion: Some(CheckConclusion::Success),
                started_at: None,
                completed_at: None,
            }],
            commit_status_state: CommitStatusState::Pending,
            commit_statuses: Vec::new(),
            reviews: vec![
                review(10, "reviewer-general"),
                review(11, "reviewer-secperf"),
            ],
        },
        threads: vec![ReviewThreadSnapshot {
            id: "PRRT_1".into(),
            is_resolved: true,
            is_outdated: false,
            path: "src/lib.rs".into(),
        }],
        issue_authorized: true,
    }
}

fn review(id: u64, role: &str) -> ReviewSnapshot {
    ReviewSnapshot {
        id,
        actor_id: match role {
            "reviewer-general" => 202_881,
            "reviewer-secperf" => 202_882,
            _ => unreachable!(),
        },
        state: ReviewState::Approved,
        commit_id: Some("b".repeat(40)),
        exact_head: true,
        submitted_at: Some(format!("2026-08-20T00:00:{id:02}Z")),
        body: format!("Independent review complete.\n\nPip reviewer role: {role}"),
    }
}

fn results() -> Vec<WorkerResult> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    fixture["results"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect()
}

fn binding(result: &WorkerResult) -> WorkerBinding {
    let common = result.common();
    let (plan_version, pr_number, expected_head_sha) = match result {
        WorkerResult::Planner(result) => (result.plan_version, None, None),
        WorkerResult::Builder(result) => (result.plan_version, None, None),
        WorkerResult::Review(result) => (
            result.plan_version,
            Some(result.pr_number),
            Some(result.reviewed_head_sha.clone()),
        ),
        WorkerResult::Final(_) => unreachable!(),
    };
    WorkerBinding {
        case: common.case.clone(),
        task_id: common.task_id.clone(),
        role: common.role,
        reviewer_id: match result {
            WorkerResult::Review(result) => Some(result.reviewer_id.clone()),
            _ => None,
        },
        review_mode: match result {
            WorkerResult::Review(_) => Some(ReviewMode::Required),
            _ => None,
        },
        requested_model: common.requested_model.clone(),
        skills_repository_commit: common.skills_repository_commit.clone(),
        plan_version,
        pr_number,
        expected_head_sha,
    }
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["repository"]["id"] = json!(984321);
    value["workflow_version"] = json!(1);
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    value["required_ci_contexts"] = json!(["test"]);
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}
