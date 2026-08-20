use pip_contracts::{WorkerBinding, WorkerResult};
use pip_control::{CiCycle, PullRequestSource, load_repository_policy, reconcile_ci_once};
use pip_controller::ingest_worker_result;
use pip_github::{
    CheckConclusion, CheckRunSnapshot, CheckStatus, CommitStatusState, GitHubError,
    PullRequestEvidence, PullRequestSnapshot,
};
use pip_store::{EffectInput, EventInput, NewCase, Store};
use serde_json::{Value, json};

struct FixtureSource {
    evidence: PullRequestEvidence,
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

#[test]
fn exact_green_ci_is_recorded_and_releases_two_reviewers() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = waiting_ci_store(directory.path().join("ledger.db"));
    let result = reconcile_ci_once(
        &FixtureSource {
            evidence: evidence(vec![check(CheckConclusion::Success)]),
        },
        &active_policy(),
        &mut store,
        100,
    )
    .unwrap();

    assert_eq!(
        result,
        CiCycle::Transitioned {
            case_key: "repo:984321#1240@1".into(),
            verdict: "ACCEPTED".into(),
        }
    );
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "REVIEWING"
    );
    assert_eq!(store.evidence_count().unwrap(), 3);
    assert_eq!(store.status(100).unwrap().outbox_pending, 1);
}

#[test]
fn pending_ci_is_read_only_and_historical_failure_enters_remediation() {
    let directory = tempfile::tempdir().unwrap();
    let mut pending_store = waiting_ci_store(directory.path().join("pending.db"));
    let mut pending = evidence(Vec::new());
    pending.check_runs.push(CheckRunSnapshot {
        status: CheckStatus::InProgress,
        conclusion: None,
        ..check(CheckConclusion::Success)
    });
    let result = reconcile_ci_once(
        &FixtureSource { evidence: pending },
        &active_policy(),
        &mut pending_store,
        100,
    )
    .unwrap();
    assert!(matches!(result, CiCycle::Pending { .. }));
    assert_eq!(
        pending_store
            .case("repo:984321#1240@1")
            .unwrap()
            .unwrap()
            .state,
        "WAITING_CI"
    );
    assert_eq!(pending_store.evidence_count().unwrap(), 2);

    let mut failed_store = waiting_ci_store(directory.path().join("failed.db"));
    let result = reconcile_ci_once(
        &FixtureSource {
            evidence: evidence(vec![
                check(CheckConclusion::Failure),
                check(CheckConclusion::Success),
            ]),
        },
        &active_policy(),
        &mut failed_store,
        101,
    )
    .unwrap();
    assert!(matches!(result, CiCycle::Transitioned { verdict, .. } if verdict == "FAILED"));
    let case = failed_store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "REMEDIATING");
    assert_eq!(case.remediation_round, 1);
}

fn waiting_ci_store(path: std::path::PathBuf) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
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
    let results = results();
    ingest_worker_result(
        &mut store,
        &active_policy().case_policy(),
        &binding(&results[0]),
        &results[0],
    )
    .unwrap();
    ingest_worker_result(
        &mut store,
        &active_policy().case_policy(),
        &binding(&results[1]),
        &results[1],
    )
    .unwrap();
    let mut now = 10;
    while let Some(effect) = store.claim_effect("fixture", now, 10).unwrap() {
        store
            .acknowledge_effect(&effect.effect_id, "fixture", now)
            .unwrap();
        now += 1;
    }
    store
}

fn evidence(check_runs: Vec<CheckRunSnapshot>) -> PullRequestEvidence {
    PullRequestEvidence {
        pull_request: PullRequestSnapshot {
            id: 9,
            number: 77,
            open: true,
            draft: true,
            merged: false,
            mergeable: Some(true),
            mergeable_state: "clean".into(),
            author_id: 10,
            head_repository_id: 984_321,
            head_repository: "marmot-protocol/mdk".into(),
            head_branch: "pip/v2/issue-1240".into(),
            head_sha: "b".repeat(40),
            base_branch: "master".into(),
            base_sha: "a".repeat(40),
        },
        check_runs,
        commit_status_state: CommitStatusState::Pending,
        commit_statuses: Vec::new(),
        reviews: Vec::new(),
    }
}

fn check(conclusion: CheckConclusion) -> CheckRunSnapshot {
    CheckRunSnapshot {
        id: match conclusion {
            CheckConclusion::Failure => 1,
            _ => 2,
        },
        app_id: 1,
        name: "test".into(),
        head_sha: "b".repeat(40),
        status: CheckStatus::Completed,
        conclusion: Some(conclusion),
        started_at: None,
        completed_at: None,
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
    let plan_version = match result {
        WorkerResult::Planner(result) => result.plan_version,
        WorkerResult::Builder(result) => result.plan_version,
        _ => unreachable!(),
    };
    WorkerBinding {
        case: common.case.clone(),
        task_id: common.task_id.clone(),
        role: common.role,
        requested_model: common.requested_model.clone(),
        skills_repository_commit: common.skills_repository_commit.clone(),
        plan_version,
        pr_number: None,
        expected_head_sha: None,
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
