use std::cell::RefCell;
use std::rc::Rc;

use pip_contracts::{WorkerResult, WorkerRole};
use pip_control::{
    DirectWorkerCycle, DirectWorkerCycleContext, DirectWorkerError, DirectWorkerRuntime,
    DirectWorkerRuntimeError, run_direct_worker_once_with,
};
use pip_controller::DirectTaskSpec;
use pip_store::{EffectInput, EventInput, NewCase, PolicyInput, Store, TransitionInput};
use serde_json::{Value, json};

#[derive(Clone)]
struct FakeRuntime {
    result: Result<WorkerResult, DirectWorkerRuntimeError>,
    tasks: Rc<RefCell<Vec<DirectTaskSpec>>>,
}

impl DirectWorkerRuntime for FakeRuntime {
    fn execute(&self, task: &DirectTaskSpec) -> Result<WorkerResult, DirectWorkerRuntimeError> {
        self.tasks.borrow_mut().push(task.clone());
        self.result.clone()
    }
}

#[test]
fn leased_direct_builder_executes_and_enters_the_shared_ingestion_path() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Ok(builder_result()));

    assert_eq!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("direct-worker-1", 100),
        )
        .unwrap(),
        DirectWorkerCycle::Ingested {
            task_id: task_id().into(),
            transition_count: 2,
        }
    );
    assert_eq!(runtime.tasks.borrow().len(), 1);
    assert_eq!(runtime.tasks.borrow()[0].model, "composer-2.5");
    assert_eq!(store.run_count().unwrap(), 1);
    assert!(store.run_by_task_id(task_id()).unwrap().is_some());
    let status = store.status(101).unwrap();
    assert_eq!(status.outbox_leased, 0);
    assert_eq!(status.outbox_superseded, 1);
}

#[test]
fn provider_failure_releases_the_job_for_immediate_retry_without_state_change() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Err(DirectWorkerRuntimeError::Unavailable(
        "fixture outage".into(),
    )));

    assert!(matches!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("direct-worker-1", 100),
        ),
        Err(DirectWorkerError::Runtime(_))
    ));
    assert_eq!(store.run_count().unwrap(), 0);
    assert_eq!(
        store.case(case_key()).unwrap().unwrap().state,
        "READY_TO_BUILD"
    );
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
    assert!(
        store
            .claim_effect_matching("direct-worker-2", 100, 30, &["RUN_DIRECT_WORKER"])
            .unwrap()
            .is_some()
    );
}

#[test]
fn stale_authorization_never_claims_or_executes_a_direct_job() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder(directory.path());
    let runtime = runtime(Ok(builder_result()));
    let mut cycle = context("direct-worker-1", 100);
    cycle.authorization_valid = false;

    assert_eq!(
        run_direct_worker_once_with(&mut store, &active_policy(), &runtime, cycle).unwrap(),
        DirectWorkerCycle::AuthorizationBlocked
    );
    assert!(runtime.tasks.borrow().is_empty());
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
}

#[test]
fn corrupted_direct_job_is_released_and_never_reaches_the_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = queued_builder_with(directory.path(), |task| {
        task["model"] = json!("auto");
    });
    let runtime = runtime(Ok(builder_result()));

    assert!(matches!(
        run_direct_worker_once_with(
            &mut store,
            &active_policy(),
            &runtime,
            context("direct-worker-1", 100),
        ),
        Err(DirectWorkerError::InvalidJob)
    ));
    assert!(runtime.tasks.borrow().is_empty());
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
}

fn context(owner: &str, now: u64) -> DirectWorkerCycleContext<'_> {
    DirectWorkerCycleContext {
        owner,
        now,
        lease_seconds: 30,
        authorization_valid: true,
    }
}

fn runtime(result: Result<WorkerResult, DirectWorkerRuntimeError>) -> FakeRuntime {
    FakeRuntime {
        result,
        tasks: Rc::new(RefCell::new(Vec::new())),
    }
}

fn active_policy() -> pip_control::RepositoryPolicy {
    let mut value: Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["intake"]["enabled"] = json!(true);
    value["intake"]["paused"] = json!(false);
    value["dispatch_enabled"] = json!(true);
    value["github"]["automation_actor_id"] = json!(202880);
    value["github"]["reviewer_general_actor_id"] = json!(202881);
    value["github"]["reviewer_secperf_actor_id"] = json!(202882);
    pip_control::load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn queued_builder(root: &std::path::Path) -> Store {
    queued_builder_with(root, |_| {})
}

fn queued_builder_with(root: &std::path::Path, mutate: impl FnOnce(&mut Value)) -> Store {
    let mut store = Store::open(root.join("ledger.db")).unwrap();
    store
        .record_policy(&PolicyInput {
            repository_id: 1_055_628_515,
            revision: 1,
            accepted_at: 1,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: case_key().into(),
            repository_id: 1_055_628_515,
            issue_number: 1240,
            workflow_version: 2,
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
    let mut task = serde_json::to_value(direct_task()).unwrap();
    mutate(&mut task);
    store
        .apply_transition(
            &TransitionInput {
                case_key: case_key().into(),
                expected_revision: 1,
                next_state: "READY_TO_BUILD".into(),
                remediation_round: 1,
                plan_version: 1,
                pr_number: None,
                head_sha: None,
                observed_at: 2,
                event: EventInput {
                    event_id: "event-plan-published".into(),
                    event_type: "PLAN_PUBLISHED".into(),
                    payload: json!({"plan_version": 1}),
                },
                run: None,
                evidence: Vec::new(),
                findings: Vec::new(),
                effects: vec![EffectInput {
                    effect_id: "effect-dispatch-builder:direct:builder".into(),
                    effect_type: "RUN_DIRECT_WORKER".into(),
                    payload: task,
                }],
            },
            None,
        )
        .unwrap();
    store
}

fn direct_task() -> DirectTaskSpec {
    DirectTaskSpec {
        schema_version: 1,
        source_effect_id: "effect-dispatch-builder".into(),
        task_id: task_id().into(),
        title: format!("Run builder for {}", case_key()),
        body: json!({
            "case_key": case_key(),
            "repository_id": 1_055_628_515_u64,
            "issue_number": 1240,
            "workflow_version": 2,
            "state_revision": 2,
            "role": "builder",
            "remediation_round": 1,
            "plan_version": 1,
            "assigned_branch": "pip/v2/repo-1055628515/issue-1240/workflow-2",
            "assigned_worktree": "/var/lib/pip-v2/worktrees/mdk/repo-1055628515-issue-1240-workflow-2",
            "execution": "direct",
            "provider": "cursor",
            "model": "composer-2.5",
            "requested_model": "cursor/composer-2.5",
            "skills_repository_commit": "a".repeat(40),
            "immutable_evidence_bundle": {"schema_version": 1, "sha256": "b".repeat(64)},
        }),
        role: WorkerRole::Builder,
        profile: "builder-grok".into(),
        workspace: "/var/lib/pip-v2/worktrees/mdk/repo-1055628515-issue-1240-workflow-2".into(),
        skills: vec!["workflow-contract".into(), "builder-grok".into()],
        provider: "cursor".into(),
        model: "composer-2.5".into(),
        max_runtime: "PT45M".into(),
        priority: 50,
    }
}

fn builder_result() -> WorkerResult {
    let mut value = serde_json::from_str::<Value>(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()["results"][1]
        .clone();
    value["workflow_version"] = json!(2);
    value["case"]["repository_id"] = json!(1_055_628_515_u64);
    value["case"]["workflow_version"] = json!(2);
    value["task_id"] = json!(task_id());
    serde_json::from_value(value).unwrap()
}

fn case_key() -> &'static str {
    "repo:1055628515#1240@2"
}

fn task_id() -> &'static str {
    "repo:1055628515#1240@2:builder:round:1:revision:2:worker"
}
