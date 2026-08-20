use std::cell::RefCell;

use pip_control::{PlanPublicationCycle, PlanWriter, load_repository_policy, publish_plan_once};
use pip_github::{CommentSpec, GitHubError, MutationResult};
use pip_store::{EffectInput, EventInput, NewCase, RunInput, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Default)]
struct FixtureWriter {
    comments: RefCell<Vec<CommentSpec>>,
    fail: bool,
}

impl PlanWriter for FixtureWriter {
    fn ensure_plan_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError> {
        self.comments.borrow_mut().push(spec.clone());
        if self.fail {
            Err(GitHubError::Transport("fixture outage".into()))
        } else {
            Ok(MutationResult::Created(10_001))
        }
    }
}

#[test]
fn proceed_plan_is_published_before_builder_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = plan_store(directory.path().join("ledger.db"), "PROCEED");
    let writer = FixtureWriter::default();

    let result = publish_plan_once(
        &writer,
        &active_policy(),
        &mut store,
        100,
        "plan-publisher",
        30,
        true,
    )
    .unwrap();

    assert_eq!(
        result,
        PlanPublicationCycle::Published {
            case_key: "repo:984321#1240@1".into(),
            plan_version: 1,
            comment_id: 10_001,
        }
    );
    let comments = writer.comments.borrow();
    assert_eq!(comments[0].issue_number, 1240);
    assert_eq!(comments[0].expected_actor_id, 202_880);
    assert!(comments[0].body.contains("## Pip plan v1: PROCEED"));
    assert!(comments[0].body.contains("Pip execution binding:"));
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "READY_TO_BUILD");
    assert_eq!(case.plan_version, 1);
    assert!(
        store
            .claim_effect_matching("dispatcher", 100, 30, &["DISPATCH_BUILDER"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn human_wait_plan_is_published_before_disposition() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = plan_store(
        directory.path().join("ledger.db"),
        "NEEDS_HUMAN_SCOPE_DECISION",
    );
    publish_plan_once(
        &FixtureWriter::default(),
        &active_policy(),
        &mut store,
        100,
        "plan-publisher",
        30,
        true,
    )
    .unwrap();

    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "WAITING_HUMAN"
    );
    assert!(
        store
            .claim_effect_matching("disposition", 100, 30, &["HOLD_FOR_HUMAN"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn publication_outage_releases_the_effect_without_advancing() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = plan_store(directory.path().join("ledger.db"), "PROCEED");
    let writer = FixtureWriter {
        fail: true,
        ..FixtureWriter::default()
    };

    assert!(
        publish_plan_once(
            &writer,
            &active_policy(),
            &mut store,
            100,
            "plan-publisher",
            30,
            true,
        )
        .is_err()
    );
    assert_eq!(
        store.case("repo:984321#1240@1").unwrap().unwrap().state,
        "PLANNING"
    );
    assert!(
        store
            .claim_effect_matching("retry", 100, 30, &["PUBLISH_PLAN"])
            .unwrap()
            .is_some()
    );
}

fn plan_store(path: std::path::PathBuf, outcome: &str) -> Store {
    let mut result = planner_fixture();
    result["outcome"] = json!(outcome);
    if outcome != "PROCEED" {
        result["open_decisions"] = json!(["human must define the authorized scope"]);
    }
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
            effects: Vec::new(),
        })
        .unwrap();
    store
        .apply_transition(
            &TransitionInput {
                case_key: "repo:984321#1240@1".into(),
                expected_revision: 1,
                next_state: "PLANNING".into(),
                remediation_round: 0,
                plan_version: 0,
                pr_number: None,
                head_sha: None,
                observed_at: 2,
                event: EventInput {
                    event_id: "event-plan-recorded".into(),
                    event_type: "PLAN_RECORDED".into(),
                    payload: result.clone(),
                },
                run: Some(RunInput {
                    run_id: "run-planner-1".into(),
                    task_id: "planner-1".into(),
                    role: "planner".into(),
                    payload: result,
                }),
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-publish-plan".into(),
                    effect_type: "PUBLISH_PLAN".into(),
                    payload: json!({"case_key":"repo:984321#1240@1"}),
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn planner_fixture() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    fixture["results"][0].clone()
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
