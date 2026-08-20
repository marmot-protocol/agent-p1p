use std::cell::RefCell;

use pip_control::{
    DraftPullRequestCycle, DraftPullRequestWriter, load_repository_policy,
    publish_draft_pull_request_once,
};
use pip_github::{GitHubError, MutationResult, PullRequestSpec};
use pip_store::{EffectInput, EventInput, NewCase, RunInput, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Default)]
struct FixtureWriter {
    specs: RefCell<Vec<PullRequestSpec>>,
    fail: bool,
}

impl DraftPullRequestWriter for FixtureWriter {
    fn ensure_draft_pull_request(
        &self,
        spec: &PullRequestSpec,
    ) -> Result<MutationResult, GitHubError> {
        self.specs.borrow_mut().push(spec.clone());
        if self.fail {
            Err(GitHubError::Transport("fixture outage".into()))
        } else {
            Ok(MutationResult::Created(77))
        }
    }
}

#[test]
fn controller_creates_deterministic_draft_pr_before_ci_observation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter::default();

    let result = publish_draft_pull_request_once(
        &writer,
        &active_policy(),
        &mut store,
        100,
        "draft-pr-publisher",
        30,
        true,
    )
    .unwrap();

    assert_eq!(
        result,
        DraftPullRequestCycle::Published {
            case_key: "repo:984321#1240@1".into(),
            pull_request_number: 77,
            head_sha: "b".repeat(40),
        }
    );
    let specs = writer.specs.borrow();
    assert_eq!(
        specs[0].head_branch,
        "pip/v2/repo-984321/issue-1240/workflow-1"
    );
    assert_eq!(specs[0].head_sha, "b".repeat(40));
    assert_eq!(specs[0].effect_id, "repo:984321#1240@1:draft-pr");
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "WAITING_CI");
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("b".repeat(40)));
    assert!(
        store
            .claim_effect_matching("ci", 100, 30, &["OBSERVE_CI"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn outage_leaves_build_recorded_and_effect_retryable() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = build_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter {
        fail: true,
        ..FixtureWriter::default()
    };

    assert!(
        publish_draft_pull_request_once(
            &writer,
            &active_policy(),
            &mut store,
            100,
            "draft-pr-publisher",
            30,
            true,
        )
        .is_err()
    );
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "BUILDING"
    );
    assert!(
        store
            .claim_effect_matching("retry", 100, 30, &["PUBLISH_DRAFT_PULL_REQUEST"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn remediation_updates_the_same_owned_pr_to_the_new_exact_head() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = remediation_store(directory.path().join("ledger.db"));
    let writer = FixtureWriter::default();

    publish_draft_pull_request_once(
        &writer,
        &active_policy(),
        &mut store,
        100,
        "draft-pr-publisher",
        30,
        true,
    )
    .unwrap();

    let spec = &writer.specs.borrow()[0];
    assert_eq!(spec.effect_id, "repo:984321#1240@1:draft-pr");
    assert_eq!(spec.head_sha, "c".repeat(40));
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.pr_number, Some(77));
    assert_eq!(case.head_sha, Some("c".repeat(40)));
    assert_eq!(case.state, "WAITING_CI");
}

fn build_store(path: std::path::PathBuf) -> Store {
    let result = builder_fixture();
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "BUILDING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-builder-active".into(),
                event_type: "BUILDER_DISPATCHED".into(),
                payload: json!({"fixture":true}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 1,
                next_state: "BUILDING".into(),
                remediation_round: 0,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 2,
                event: EventInput {
                    event_id: "event-build-recorded".into(),
                    event_type: "BUILD_RECORDED".into(),
                    payload: result.clone(),
                },
                run: Some(RunInput {
                    run_id: "run-builder-1".into(),
                    task_id: "builder-1".into(),
                    role: "builder".into(),
                    payload: result,
                }),
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-publish-draft-pr".into(),
                    effect_type: "PUBLISH_DRAFT_PULL_REQUEST".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn remediation_store(path: std::path::PathBuf) -> Store {
    let mut result = builder_fixture();
    result["task_id"] = json!("builder-2");
    result["build_round"] = json!(2);
    result["head_sha"] = json!("c".repeat(40));
    let mut store = Store::open(path).unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:984321#1240@1".into(),
            repository_id: 984_321,
            issue_number: 1240,
            workflow_version: 1,
            policy_revision: 1,
            initial_state: "REMEDIATING".into(),
            observed_at: 1,
            event: EventInput {
                event_id: "event-remediation".into(),
                event_type: "REQUEST_CHANGES".into(),
                payload: json!({"fixture":true}),
            },
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 1,
                next_state: "REMEDIATING".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: Some(77),
                head_sha: Some("b".repeat(40)),
                observed_at: 2,
                event: EventInput {
                    event_id: "event-build-recorded-2".into(),
                    event_type: "BUILD_RECORDED".into(),
                    payload: result.clone(),
                },
                run: Some(RunInput {
                    run_id: "run-builder-2".into(),
                    task_id: "builder-2".into(),
                    role: "builder".into(),
                    payload: result,
                }),
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-publish-draft-pr-2".into(),
                    effect_type: "PUBLISH_DRAFT_PULL_REQUEST".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn builder_fixture() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    fixture["results"][1].clone()
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
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}
