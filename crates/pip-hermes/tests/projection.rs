use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use pip_hermes::{
    CommandOutput, CommandRunner, CommandSpec, HermesError, HermesProjector, ProjectionError,
    ProjectionResult, TaskCreateSpec, TaskSnapshot,
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

fn spec() -> TaskCreateSpec {
    TaskCreateSpec {
        board: "pip-mdk".into(),
        effect_id: "effect:repo:984321:issue:1240:plan:1".into(),
        projection_key: "repo:984321#1240@1:planner:1".into(),
        title: "Plan issue 1240".into(),
        body: json!({
            "case_key": "repo:984321#1240@1",
            "repository_id": 984321,
            "issue_number": 1240,
            "policy_revision": 1,
            "state_revision": 1
        }),
        assignee: "planner".into(),
        workspace: "scratch".into(),
        skills: vec!["workflow-contract".into(), "planner".into()],
        provider: "openai-codex".into(),
        model: "gpt-5.6-sol".into(),
        max_runtime: "30m".into(),
        priority: 10,
        parent_task_ids: Vec::new(),
    }
}

fn task(id: &str, title: &str, key: &str) -> TaskSnapshot {
    TaskSnapshot {
        id: id.into(),
        title: title.into(),
        status: "blocked".into(),
        assignee: Some("planner".into()),
        created_by: Some("pip-controller".into()),
        body: json!({"projection_key": key}).to_string(),
    }
}

#[test]
fn missing_projection_creates_one_blocked_idempotent_task() {
    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(CommandOutput {
        status: 0,
        stdout: serde_json::to_vec(&task(
            "task-1",
            "Plan issue 1240",
            "repo:984321#1240@1:planner:1",
        ))
        .unwrap(),
        stderr: Vec::new(),
        timed_out: false,
    });
    let projector =
        HermesProjector::new(runner.clone(), "hermes", Duration::from_secs(2), 4096).unwrap();

    assert_eq!(
        projector.project(&spec(), &[]).unwrap(),
        ProjectionResult::Created("task-1".into())
    );
    let command = &runner.commands.borrow()[0];
    assert_eq!(
        &command.args[..4],
        ["kanban", "--board", "pip-mdk", "create"]
    );
    let expected = spec();
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| { pair[0] == "--idempotency-key" && pair[1] == expected.effect_id })
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--initial-status", "blocked"])
    );
    assert!(!command.args.iter().any(|arg| arg == "complete"));
}

#[test]
fn exact_existing_projection_is_a_noop() {
    let runner = FakeRunner::default();
    let projector =
        HermesProjector::new(runner.clone(), "hermes", Duration::from_secs(2), 4096).unwrap();
    let observed = vec![task(
        "task-1",
        "Plan issue 1240",
        "repo:984321#1240@1:planner:1",
    )];
    assert_eq!(
        projector.project(&spec(), &observed).unwrap(),
        ProjectionResult::Existing("task-1".into())
    );
    assert!(runner.commands.borrow().is_empty());
}

#[test]
fn exact_projection_remains_owned_after_its_gate_advances_status() {
    let runner = FakeRunner::default();
    let projector =
        HermesProjector::new(runner.clone(), "hermes", Duration::from_secs(2), 4096).unwrap();
    for status in ["ready", "in_progress", "done"] {
        let mut observed = task("task-1", "Plan issue 1240", "repo:984321#1240@1:planner:1");
        observed.status = status.into();
        assert_eq!(
            projector.project(&spec(), &[observed]).unwrap(),
            ProjectionResult::Existing("task-1".into())
        );
    }
    assert!(runner.commands.borrow().is_empty());
}

#[test]
fn duplicate_or_drifted_projection_fails_without_writing() {
    let runner = FakeRunner::default();
    let projector =
        HermesProjector::new(runner.clone(), "hermes", Duration::from_secs(2), 4096).unwrap();
    let duplicate = vec![
        task("task-1", "Plan issue 1240", &spec().projection_key),
        task("task-2", "Plan issue 1240", &spec().projection_key),
    ];
    assert!(matches!(
        projector.project(&spec(), &duplicate),
        Err(ProjectionError::DuplicateProjection)
    ));
    let drifted = vec![task("task-1", "Wrong", &spec().projection_key)];
    assert!(matches!(
        projector.project(&spec(), &drifted),
        Err(ProjectionError::ProjectionDrift)
    ));
    assert!(runner.commands.borrow().is_empty());
}

#[test]
fn malformed_create_result_cannot_be_accepted() {
    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(CommandOutput {
        status: 0,
        stdout: b"{}".to_vec(),
        stderr: Vec::new(),
        timed_out: false,
    });
    let projector = HermesProjector::new(runner, "hermes", Duration::from_secs(2), 4096).unwrap();
    assert!(matches!(
        projector.project(&spec(), &[]),
        Err(ProjectionError::MalformedResult(_))
    ));
}
