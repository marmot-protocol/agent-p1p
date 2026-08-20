use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use pip_control::{
    DispatchCycleContext, DispatchCycleResult, dispatch_once_with, load_repository_policy,
};
use pip_hermes::{CommandOutput, CommandRunner, CommandSpec, HermesError, TaskSnapshot};
use pip_store::{EffectInput, EventInput, NewCase, PolicyInput, Store};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<CommandOutput>>>,
    commands: Rc<RefCell<Vec<CommandSpec>>>,
}

impl FakeRunner {
    fn output(&self, status: i32, value: impl Into<Vec<u8>>) {
        self.outputs.borrow_mut().push_back(CommandOutput {
            status,
            stdout: value.into(),
            stderr: Vec::new(),
            timed_out: false,
        });
    }

    fn json(&self, value: &impl serde::Serialize) {
        self.output(0, serde_json::to_vec(value).unwrap());
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        self.commands.borrow_mut().push(spec.clone());
        Ok(self.outputs.borrow_mut().pop_front().unwrap())
    }
}

#[test]
fn planner_dispatch_projects_gate_and_worker_then_atomically_acks_outbox() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let policy = active_policy();
    let runner = creation_runner();

    let report = dispatch_once_with(
        &mut store,
        &policy,
        runner.clone(),
        context("controller-1", 100),
    )
    .unwrap();
    assert_eq!(
        report,
        DispatchCycleResult::Projected {
            effect_id: "effect-intake-planner".into(),
            projection_count: 2,
            released_gate_count: 1,
            ledger_result: "applied".into(),
        }
    );
    let status = store.status(101).unwrap();
    assert_eq!(status.task_projections, 2);
    assert_eq!(status.outbox_delivered, 1);
    assert!(
        runner
            .commands
            .borrow()
            .iter()
            .any(|command| { command.args.iter().any(|argument| argument == "complete") })
    );

    let idle_runner = FakeRunner::default();
    assert_eq!(
        dispatch_once_with(
            &mut store,
            &policy,
            idle_runner.clone(),
            context("controller-2", 102),
        )
        .unwrap(),
        DispatchCycleResult::Idle
    );
    assert!(idle_runner.commands.borrow().is_empty());
}

#[test]
fn crash_after_gate_release_reconciles_existing_advanced_tasks_without_duplicates() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let policy = active_policy();
    let first = creation_runner();
    first.outputs.borrow_mut().back_mut().unwrap().status = 2;
    assert!(dispatch_once_with(&mut store, &policy, first, context("controller-1", 100),).is_err());
    assert_eq!(store.task_projection_count().unwrap(), 0);

    let gate = gate("done");
    let worker = worker("ready");
    let recovered = FakeRunner::default();
    recovered.json(&vec![gate.clone(), worker.clone()]);
    recovered.json(&gate);
    recovered.json(&worker);
    let report = dispatch_once_with(
        &mut store,
        &policy,
        recovered.clone(),
        context("controller-2", 131),
    )
    .unwrap();
    assert!(matches!(report, DispatchCycleResult::Projected { .. }));
    assert_eq!(store.task_projection_count().unwrap(), 2);
    assert!(recovered.commands.borrow().iter().all(|command| {
        !command.args.iter().any(|argument| argument == "create")
            && !command.args.iter().any(|argument| argument == "complete")
    }));
}

#[test]
fn invalid_fresh_authorization_never_claims_an_outbox_effect_or_calls_hermes() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = seeded_store(directory.path());
    let policy = active_policy();
    let runner = FakeRunner::default();
    let mut dispatch = context("controller-1", 100);
    dispatch.authorization_valid = false;

    assert_eq!(
        dispatch_once_with(&mut store, &policy, runner.clone(), dispatch).unwrap(),
        DispatchCycleResult::AuthorizationBlocked
    );
    assert!(runner.commands.borrow().is_empty());
    let status = store.status(100).unwrap();
    assert_eq!(status.outbox_pending, 1);
    assert_eq!(status.outbox_leased, 0);
}

fn context(owner: &str, now: u64) -> DispatchCycleContext<'_> {
    DispatchCycleContext {
        skills_repository_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        hermes_program: "hermes",
        owner,
        now,
        lease_seconds: 30,
        authorization_valid: true,
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
    load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn seeded_store(root: &std::path::Path) -> Store {
    let mut store = Store::open(root.join("ledger.db")).unwrap();
    store
        .record_policy(&PolicyInput {
            repository_id: 1_055_628_515,
            revision: 1,
            accepted_at: 90,
            payload: json!({"fixture": true}),
        })
        .unwrap();
    store
        .create_case(&NewCase {
            case_key: "repo:1055628515#1240@2".into(),
            repository_id: 1_055_628_515,
            issue_number: 1240,
            workflow_version: 2,
            policy_revision: 1,
            initial_state: "PLANNING".into(),
            observed_at: 90,
            event: EventInput {
                event_id: "event-intake-5050325280".into(),
                event_type: "ISSUE_AUTHORIZED".into(),
                payload: json!({"label": "pip-ok"}),
            },
            effects: vec![EffectInput {
                effect_id: "effect-intake-planner".into(),
                effect_type: "DISPATCH_PLANNER".into(),
                payload: json!({"case_key": "repo:1055628515#1240@2"}),
            }],
        })
        .unwrap();
    store
}

fn creation_runner() -> FakeRunner {
    let runner = FakeRunner::default();
    let gate = gate("blocked");
    let worker = worker("blocked");
    runner.json(&Vec::<TaskSnapshot>::new());
    runner.json(&gate);
    runner.json(&gate);
    runner.json(&worker);
    runner.json(&worker);
    runner.output(0, Vec::new());
    runner
}

fn gate(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "gate-1".into(),
        title: "Activate planner for repo:1055628515#1240@2".into(),
        status: status.into(),
        assignee: None,
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:1055628515#1240@2",
            "state_revision": 1,
            "activation_gate": "planner",
            "projection_key": "repo:1055628515#1240@2:planner:round:1:revision:1:gate"
        })
        .to_string(),
    }
}

fn worker(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "worker-1".into(),
        title: "Run planner for repo:1055628515#1240@2".into(),
        status: status.into(),
        assignee: Some("planner".into()),
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:1055628515#1240@2",
            "repository_id": 1055628515_u64,
            "issue_number": 1240,
            "workflow_version": 2,
            "state_revision": 1,
            "role": "planner",
            "remediation_round": 0,
            "execution": "hermes",
            "provider": "openai-codex",
            "model": "gpt-5.6-sol",
            "projection_key": "repo:1055628515#1240@2:planner:round:1:revision:1:worker"
        })
        .to_string(),
    }
}
