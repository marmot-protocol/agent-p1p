use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use pip_hermes::{
    CommandOutput, CommandRunner, CommandSpec, GateCreateSpec, GateError, GateProjectionResult,
    GateReleaseResult, HermesError, HermesGateController, TaskSnapshot,
};
use serde_json::json;

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<CommandOutput>>>,
    commands: Rc<RefCell<Vec<CommandSpec>>>,
}

impl CommandRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        self.commands.borrow_mut().push(spec.clone());
        Ok(self.outputs.borrow_mut().pop_front().unwrap())
    }
}

fn spec() -> GateCreateSpec {
    GateCreateSpec {
        board: "pip-mdk".into(),
        effect_id: "effect:repo:984321:issue:1240:gate:planner:1".into(),
        projection_key: "repo:984321#1240@1:gate:planner:1".into(),
        title: "Activate planner for repo:984321#1240@1".into(),
        body: json!({
            "case_key": "repo:984321#1240@1",
            "state_revision": 1,
            "activation_gate": "planner"
        }),
        parent_task_ids: Vec::new(),
    }
}

fn gate(status: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: "gate-1".into(),
        title: spec().title,
        status: status.into(),
        assignee: None,
        created_by: Some("pip-controller".into()),
        body: json!({
            "case_key": "repo:984321#1240@1",
            "state_revision": 1,
            "activation_gate": "planner",
            "projection_key": spec().projection_key
        })
        .to_string(),
    }
}

fn controller(runner: FakeRunner) -> HermesGateController<FakeRunner> {
    HermesGateController::new(runner, "hermes", Duration::from_secs(2), 4096).unwrap()
}

#[test]
fn gate_projection_is_blocked_idempotent_and_has_no_worker_profile() {
    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(CommandOutput {
        status: 0,
        stdout: serde_json::to_vec(&gate("blocked")).unwrap(),
        stderr: Vec::new(),
        timed_out: false,
    });
    assert_eq!(
        controller(runner.clone()).project(&spec(), &[]).unwrap(),
        GateProjectionResult::Created("gate-1".into())
    );
    let command = &runner.commands.borrow()[0];
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--initial-status", "blocked"])
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| { pair[0] == "--idempotency-key" && pair[1] == spec().effect_id })
    );
    assert!(!command.args.iter().any(|arg| arg == "--assignee"));
    assert!(!command.args.iter().any(|arg| arg == "--model"));

    assert_eq!(
        controller(FakeRunner::default())
            .project(&spec(), &[gate("blocked")])
            .unwrap(),
        GateProjectionResult::Existing("gate-1".into())
    );
}

#[test]
fn only_an_exact_blocked_gate_can_be_released_by_the_controller() {
    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(CommandOutput {
        status: 0,
        stdout: Vec::new(),
        stderr: Vec::new(),
        timed_out: false,
    });
    assert_eq!(
        controller(runner.clone())
            .release(&spec(), &gate("blocked"), "authorization revalidated")
            .unwrap(),
        GateReleaseResult::Released
    );
    assert_eq!(
        runner.commands.borrow()[0].args,
        [
            "kanban",
            "--board",
            "pip-mdk",
            "complete",
            "gate-1",
            "--result",
            "authorization revalidated",
        ]
    );

    assert_eq!(
        controller(FakeRunner::default())
            .release(&spec(), &gate("done"), "replayed")
            .unwrap(),
        GateReleaseResult::AlreadyReleased
    );
    assert!(matches!(
        controller(FakeRunner::default()).release(&spec(), &gate("ready"), "unsafe"),
        Err(GateError::UnsafeGateState)
    ));
}

#[test]
fn duplicate_or_drifted_gates_fail_without_commands() {
    let runner = FakeRunner::default();
    let mut drifted = gate("blocked");
    drifted.created_by = Some("worker".into());
    assert!(matches!(
        controller(runner.clone()).project(&spec(), &[drifted]),
        Err(GateError::ProjectionDrift)
    ));
    assert!(matches!(
        controller(runner.clone()).project(&spec(), &[gate("blocked"), gate("blocked")]),
        Err(GateError::DuplicateProjection)
    ));
    assert!(runner.commands.borrow().is_empty());
}
