use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use pip_control::{ResultCycle, ingest_completed_once_with, load_repository_policy};
use pip_hermes::{
    CommandOutput, CommandRunner, CommandSpec, HermesError, TaskCreateSpec, TaskSnapshot,
};
use pip_store::{EffectInput, EventInput, NewCase, Store, TaskProjectionInput};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<CommandOutput>>>,
}

impl FakeRunner {
    fn json(&self, value: Value) {
        self.outputs.borrow_mut().push_back(CommandOutput {
            status: 0,
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: Vec::new(),
            timed_out: false,
        });
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, _spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        Ok(self.outputs.borrow_mut().pop_front().unwrap())
    }
}

#[test]
fn completed_hermes_result_is_bound_to_the_owned_projection_and_ingested_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    project_planner(&mut store);
    let runner = FakeRunner::default();
    runner.json(completed_planner("planner", planner_result()));

    let first =
        ingest_completed_once_with(&mut store, &active_policy(), runner, "hermes", 10).unwrap();
    assert_eq!(
        first,
        ResultCycle::Ingested {
            task_id: "planner-1".into(),
            transition_count: 1,
        }
    );
    assert_eq!(store.run_count().unwrap(), 1);
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "PLANNING");
    assert_eq!(case.plan_version, 0);
    let status = store.status(10).unwrap();
    assert_eq!(status.outbox_pending, 1);

    assert_eq!(
        ingest_completed_once_with(
            &mut store,
            &active_policy(),
            FakeRunner::default(),
            "hermes",
            11,
        )
        .unwrap(),
        ResultCycle::Idle
    );
}

#[test]
fn stock_hermes_completion_annotations_are_not_worker_contract_fields() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    project_planner(&mut store);
    let mut result = planner_result();
    result["worker_session_id"] = json!("20260905_083128_12a0bd");
    result["artifacts"] = json!(["/isolated/plans/plan-v1.md"]);
    result["_staged_artifacts"] = json!(["/isolated/attachments/plan-v1.md"]);
    let runner = FakeRunner::default();
    runner.json(completed_planner("planner", result));
    assert!(matches!(
        ingest_completed_once_with(&mut store, &active_policy(), runner, "hermes", 10).unwrap(),
        ResultCycle::Ingested {
            transition_count: 1,
            ..
        }
    ));
    assert_eq!(store.run_count().unwrap(), 1);
}

#[test]
fn transport_annotations_never_hide_unknown_fields_bad_shapes_or_binding_drift() {
    for (field, value) in [
        ("invented_contract_field", json!(true)),
        ("_invented_transport_field", json!(true)),
        ("worker_session_id", json!({"forged": true})),
        ("worker_session_id", json!("")),
        ("artifacts", json!({"path": "/tmp/result"})),
        ("artifacts", json!([false])),
        ("_staged_artifacts", json!("/tmp/result")),
        ("task_id", json!("foreign-task")),
        ("actual_model", json!("openai-codex/auto")),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        project_planner(&mut store);
        let mut result = planner_result();
        result["worker_session_id"] = json!("20260905_083128_12a0bd");
        result[field] = value;
        let runner = FakeRunner::default();
        runner.json(completed_planner("planner", result));
        assert!(
            ingest_completed_once_with(&mut store, &active_policy(), runner, "hermes", 10,)
                .is_err(),
            "accepted {field}"
        );
        assert_eq!(store.run_count().unwrap(), 0);
    }
}

#[test]
fn wrong_profile_or_self_described_model_never_mutates_the_case() {
    for (profile, mutate) in [("foreign", false), ("planner", true)] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        project_planner(&mut store);
        let mut result = planner_result();
        if mutate {
            result["requested_model"] = json!("openai-codex/auto");
            result["actual_model"] = json!("openai-codex/auto");
        }
        let runner = FakeRunner::default();
        runner.json(completed_planner(profile, result));

        assert!(
            ingest_completed_once_with(&mut store, &active_policy(), runner, "hermes", 10).is_err()
        );
        assert_eq!(store.run_count().unwrap(), 0);
        assert_eq!(
            store.case("repo:984321#1240@1").unwrap().unwrap().state,
            "PLANNING"
        );
    }
}

#[test]
fn a_hermes_circuit_breaker_escalates_the_case_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
    project_planner(&mut store);
    let runner = FakeRunner::default();
    runner.json(json!({
        "task": {
            "id": "planner-1",
            "title": "Run planner",
            "status": "blocked",
            "assignee": "planner",
            "created_by": "pip-controller",
            "body": serde_json::to_string(&json!({
                "projection_key": "repo:984321#1240@1:planner:round:1:revision:1:worker"
            })).unwrap()
        },
        "runs": [
            {"outcome":"spawn_failed","profile":"planner","metadata":{}},
            {"outcome":"spawn_failed","profile":"planner","metadata":{}},
            {"outcome":"gave_up","profile":"planner","metadata":{"failures":3}}
        ]
    }));

    assert_eq!(
        ingest_completed_once_with(&mut store, &active_policy(), runner, "hermes", 50).unwrap(),
        ResultCycle::ProviderFailureEscalated {
            task_id: "planner-1".into()
        }
    );
    let case = store.case("repo:984321#1240@1").unwrap().unwrap();
    assert_eq!(case.state, "ESCALATED");
    assert_eq!(
        store.latest_event_type(&case.case_key).unwrap().as_deref(),
        Some("OPERATIONAL_BOUND_REACHED")
    );
}

fn project_planner(store: &mut Store) {
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
    store.claim_effect("dispatch", 2, 30).unwrap().unwrap();
    let desired = TaskCreateSpec {
        board: "pip-mdk".into(),
        effect_id: "effect-planner:worker".into(),
        projection_key: "repo:984321#1240@1:planner:round:1:revision:1:worker".into(),
        title: "Run planner".into(),
        body: json!({
            "case_key":"repo:984321#1240@1",
            "repository_id":984321,
            "issue_number":1240,
            "workflow_version":1,
            "state_revision":1,
            "role":"planner",
            "plan_version":1,
            "requested_model":"openai-codex/gpt-5.6-sol"
            ,"skills_repository_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }),
        assignee: "planner".into(),
        workspace: "/tmp/worktree".into(),
        skills: vec!["workflow-contract".into(), "planner".into()],
        provider: "openai-codex".into(),
        model: "gpt-5.6-sol".into(),
        max_runtime: "PT30M".into(),
        max_retries: 3,
        priority: 50,
        parent_task_ids: vec!["gate-1".into()],
    };
    let observed = TaskSnapshot {
        configuration: Default::default(),
        id: "planner-1".into(),
        title: desired.title.clone(),
        status: "ready".into(),
        assignee: Some("planner".into()),
        created_by: Some("pip-controller".into()),
        body: "{}".into(),
    };
    store
        .complete_task_projection(
            &TaskProjectionInput {
                projection_id: desired.projection_key.clone(),
                effect_id: "effect-planner".into(),
                board: "pip-mdk".into(),
                task_id: "planner-1".into(),
                desired: serde_json::to_value(desired).unwrap(),
                observed: serde_json::to_value(observed).unwrap(),
            },
            "dispatch",
            3,
            None,
        )
        .unwrap();
}

fn planner_result() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap();
    fixture["results"][0].clone()
}

fn completed_planner(profile: &str, metadata: Value) -> Value {
    let projection_key = "repo:984321#1240@1:planner:round:1:revision:1:worker";
    json!({
        "task": {
            "id":"planner-1",
            "title":"Run planner",
            "status":"done",
            "assignee":"planner",
            "created_by":"pip-controller",
            "body": serde_json::to_string(&json!({"projection_key": projection_key})).unwrap()
        },
        "runs":[{"outcome":"completed","profile":profile,"metadata":metadata}]
    })
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
