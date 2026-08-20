use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{WorkerBinding, WorkerResult};
use pip_controller::{
    IngestError, IngestResult, LedgerController, WorkflowCommand, ingest_worker_result,
};
use pip_core::{
    CaseId, CasePolicy, CaseState, Event, EventId, GitSha, IssueNumber, MergeMode, ObservedAt,
    PlanVersion, PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::{EffectInput, EventInput, NewCase, Store};
use serde_json::{Value, json};

#[test]
fn complete_worker_sequence_is_immutable_exact_head_and_shadow_held() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    create_case(&mut store);
    let results = results();

    assert_eq!(
        ingest_worker_result(&mut store, &policy(), &binding(&results[0]), &results[0],).unwrap(),
        IngestResult::Applied {
            transition_count: 1
        }
    );
    assert_eq!(case(&store).state, "PLANNING");
    assert_eq!(case(&store).plan_version, 0);
    publish_plan(&mut store, &results[0]);
    assert_eq!(case(&store).state, "READY_TO_BUILD");

    assert_eq!(
        ingest_worker_result(&mut store, &policy(), &binding(&results[1]), &results[1],).unwrap(),
        IngestResult::Applied {
            transition_count: 2
        }
    );
    assert_eq!(case(&store).state, "BUILDING");
    publish_build(&mut store, &results[1]);
    assert_eq!(case(&store).state, "WAITING_CI");
    accept_ci(&mut store);
    assert_eq!(case(&store).state, "REVIEWING");

    ingest_worker_result(&mut store, &policy(), &binding(&results[2]), &results[2]).unwrap();
    assert_eq!(case(&store).state, "REVIEWING");
    assert_eq!(store.run_count().unwrap(), 3);

    ingest_worker_result(&mut store, &policy(), &binding(&results[3]), &results[3]).unwrap();
    assert_eq!(case(&store).state, "FINAL_REVIEW");
    assert_eq!(store.run_count().unwrap(), 4);

    assert!(matches!(
        ingest_worker_result(&mut store, &policy(), &binding(&results[4]), &results[4]),
        Err(IngestError::InvalidState)
    ));
    assert_eq!(store.run_count().unwrap(), 4);
    accept_final_preflight(&mut store);
    ingest_worker_result(&mut store, &policy(), &binding(&results[4]), &results[4]).unwrap();
    assert_eq!(case(&store).state, "SHADOW_READY");
    assert_eq!(store.run_count().unwrap(), 5);
    assert_eq!(store.finding_count().unwrap(), 0);
    assert_eq!(store.evidence_count().unwrap(), 5);

    assert_eq!(
        ingest_worker_result(&mut store, &policy(), &binding(&results[2]), &results[2],).unwrap(),
        IngestResult::Replayed
    );
    assert_eq!(store.run_count().unwrap(), 5);
    assert_eq!(case(&store).state, "SHADOW_READY");
}

#[test]
fn binding_mismatch_fails_without_ledger_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    create_case(&mut store);
    let result = results().remove(0);
    let mut wrong = binding(&result);
    wrong.requested_model = "auto".into();

    assert!(matches!(
        ingest_worker_result(&mut store, &policy(), &wrong, &result),
        Err(IngestError::Contract(_))
    ));
    assert_eq!(store.run_count().unwrap(), 0);
    assert_eq!(store.event_count().unwrap(), 1);
    assert_eq!(case(&store).state, "PLANNING");
}

#[test]
fn unexpected_planner_model_blocks_without_publishing_untrusted_output() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    create_case(&mut store);
    let mut value = serde_json::to_value(&results()[0]).unwrap();
    value["outcome"] = json!("BLOCKED_UNEXPECTED_MODEL");
    value["actual_model"] = json!("openai-codex/substituted");
    let result: WorkerResult = serde_json::from_value(value).unwrap();

    ingest_worker_result(&mut store, &policy(), &binding(&result), &result).unwrap();

    assert_eq!(case(&store).state, "BLOCKED");
    assert!(
        store
            .claim_effect_matching("recorder", 10, 30, &["RECORD_BLOCK"])
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .claim_effect_matching("publisher", 10, 30, &["PUBLISH_PLAN"])
            .unwrap()
            .is_none()
    );
}

#[test]
fn two_reviews_join_to_request_changes_and_preserve_both_runs_and_findings() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    create_case(&mut store);
    let mut results = results();
    ingest_worker_result(&mut store, &policy(), &binding(&results[0]), &results[0]).unwrap();
    publish_plan(&mut store, &results[0]);
    ingest_worker_result(&mut store, &policy(), &binding(&results[1]), &results[1]).unwrap();
    publish_build(&mut store, &results[1]);
    accept_ci(&mut store);

    let value = serde_json::to_value(&results[2]).unwrap();
    let mut changed = value;
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

    ingest_worker_result(&mut store, &policy(), &binding(&results[2]), &results[2]).unwrap();
    assert_eq!(case(&store).state, "REVIEWING");
    ingest_worker_result(&mut store, &policy(), &binding(&results[3]), &results[3]).unwrap();

    assert_eq!(case(&store).state, "REMEDIATING");
    assert_eq!(case(&store).remediation_round, 1);
    assert_eq!(store.run_count().unwrap(), 4);
    assert_eq!(store.finding_count().unwrap(), 1);
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
        WorkerResult::Final(result) => (
            result.plan_version,
            Some(result.pr_number),
            Some(result.reviewed_head_sha.clone()),
        ),
    };
    WorkerBinding {
        case: common.case.clone(),
        task_id: common.task_id.clone(),
        role: common.role,
        requested_model: common.requested_model.clone(),
        skills_repository_commit: common.skills_repository_commit.clone(),
        plan_version,
        pr_number,
        expected_head_sha,
    }
}

fn create_case(store: &mut Store) {
    store
        .create_case(&NewCase {
            case_key: case_id().to_string(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-intake".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label": "pip-ok"}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-intake-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key": case_id().to_string()}),
            }],
        })
        .unwrap();
}

fn accept_ci(store: &mut Store) {
    let current = case(store);
    let workflow = WorkflowCommand {
        case_id: case_id(),
        event_id: EventId::from_str("event-ci-accepted").unwrap(),
        observed_at: ObservedAt::new(20),
        expected_state: CaseState::WaitingCi,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(current.state_revision).unwrap(),
        ),
        accepted_policy_revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        remediation_round: current.remediation_round,
        plan_version: Some(PlanVersion::new(NonZeroU32::new(1).unwrap())),
        pr_number: Some(PullRequestNumber::new(NonZeroU64::new(77).unwrap())),
        head_sha: Some(GitSha::from_str(&"b".repeat(40)).unwrap()),
        event: Event::CiAccepted,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({"head_sha": "b".repeat(40)}),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    };
    LedgerController::apply(store, &policy(), &workflow).unwrap();
}

fn publish_plan(store: &mut Store, result: &WorkerResult) {
    let WorkerResult::Planner(planner) = result else {
        panic!("planner result required");
    };
    let current = case(store);
    let workflow = WorkflowCommand {
        case_id: case_id(),
        event_id: EventId::from_str("event-plan-published").unwrap(),
        observed_at: ObservedAt::new(10),
        expected_state: CaseState::Planning,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(current.state_revision).unwrap(),
        ),
        accepted_policy_revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        remediation_round: current.remediation_round,
        plan_version: None,
        pr_number: None,
        head_sha: None,
        event: Event::Proceed,
        accepted_plan_version: Some(PlanVersion::new(
            NonZeroU32::new(planner.plan_version).unwrap(),
        )),
        next_pr_number: None,
        next_head_sha: None,
        event_payload: serde_json::to_value(planner).unwrap(),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    };
    LedgerController::apply(store, &policy(), &workflow).unwrap();
}

fn publish_build(store: &mut Store, result: &WorkerResult) {
    let WorkerResult::Builder(build) = result else {
        panic!("builder result required");
    };
    let current = case(store);
    let workflow = WorkflowCommand {
        case_id: case_id(),
        event_id: EventId::from_str(&format!("event-draft-pr-published-{}", build.build_round))
            .unwrap(),
        observed_at: ObservedAt::new(11),
        expected_state: CaseState::from_str(&current.state).unwrap(),
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(current.state_revision).unwrap(),
        ),
        accepted_policy_revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        remediation_round: current.remediation_round,
        plan_version: Some(PlanVersion::new(NonZeroU32::new(1).unwrap())),
        pr_number: current
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: current
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .unwrap(),
        event: Event::ReviewReady,
        accepted_plan_version: None,
        next_pr_number: Some(PullRequestNumber::new(NonZeroU64::new(77).unwrap())),
        next_head_sha: build
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .unwrap(),
        event_payload: json!({"builder_result": build}),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    };
    LedgerController::apply(store, &policy(), &workflow).unwrap();
}

fn accept_final_preflight(store: &mut Store) {
    publish_reviews(store);
    let current = case(store);
    let workflow = WorkflowCommand {
        case_id: case_id(),
        event_id: EventId::from_str("event-final-preflight-accepted").unwrap(),
        observed_at: ObservedAt::new(30),
        expected_state: CaseState::FinalReview,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(current.state_revision).unwrap(),
        ),
        accepted_policy_revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        remediation_round: current.remediation_round,
        plan_version: Some(PlanVersion::new(NonZeroU32::new(1).unwrap())),
        pr_number: Some(PullRequestNumber::new(NonZeroU64::new(77).unwrap())),
        head_sha: Some(GitSha::from_str(&"b".repeat(40)).unwrap()),
        event: Event::FinalPreflightAccepted,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({"verdict": "ACCEPTED"}),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    };
    LedgerController::apply(store, &policy(), &workflow).unwrap();
}

fn publish_reviews(store: &mut Store) {
    let current = case(store);
    let workflow = WorkflowCommand {
        case_id: case_id(),
        event_id: EventId::from_str("event-reviews-published").unwrap(),
        observed_at: ObservedAt::new(29),
        expected_state: CaseState::FinalReview,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(current.state_revision).unwrap(),
        ),
        accepted_policy_revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        remediation_round: current.remediation_round,
        plan_version: Some(PlanVersion::new(NonZeroU32::new(1).unwrap())),
        pr_number: Some(PullRequestNumber::new(NonZeroU64::new(77).unwrap())),
        head_sha: Some(GitSha::from_str(&"b".repeat(40)).unwrap()),
        event: Event::ReviewsPublished,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({"fixture": true}),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    };
    LedgerController::apply(store, &policy(), &workflow).unwrap();
}

fn case(store: &Store) -> pip_store::StoredCase {
    store.case(&case_id().to_string()).unwrap().unwrap()
}

fn case_id() -> CaseId {
    CaseId::new(
        RepositoryId::new(NonZeroU64::new(984_321).unwrap()),
        IssueNumber::new(NonZeroU64::new(1240).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
    )
}

fn policy() -> CasePolicy {
    CasePolicy {
        revision: PolicyRevision::new(NonZeroU64::new(1).unwrap()),
        merge_mode: MergeMode::Shadow,
        max_remediation_rounds: NonZeroU32::new(3).unwrap(),
    }
}
