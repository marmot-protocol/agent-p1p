use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use pip_hermes::{
    CommandOutput, CommandRunner, CommandSpec, DesiredTask, Discrepancy, HermesError, HermesReader,
    ProcessRunner, compare_projection,
};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<Result<CommandOutput, HermesError>>>>,
    commands: Rc<RefCell<Vec<CommandSpec>>>,
}

impl FakeRunner {
    fn output(&self, stdout: &str) {
        self.outputs.borrow_mut().push_back(Ok(CommandOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            timed_out: false,
        }));
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        self.commands.borrow_mut().push(spec.clone());
        self.outputs.borrow_mut().pop_front().unwrap()
    }
}

fn reader(runner: FakeRunner) -> HermesReader<FakeRunner> {
    HermesReader::new(runner, "hermes", Duration::from_secs(2), 4096).unwrap()
}

#[test]
fn capability_board_and_task_reads_use_only_read_commands() {
    let runner = FakeRunner::default();
    runner.output("hermes 0.9.0\n");
    runner.output(r#"[{"name":"pip-mdk"},{"name":"pip-other"}]"#);
    runner.output(
        r#"[{"id":"task-1","title":"Plan","status":"ready","assignee":"planner","created_by":"pip-controller","body":"{\"projection_key\":\"plan-1\"}"}]"#,
    );
    runner.output(
        r#"{"id":"task-1","title":"Plan","status":"ready","assignee":"planner","created_by":"pip-controller","body":"{\"projection_key\":\"plan-1\"}"}"#,
    );

    let reader = reader(runner.clone());
    let capabilities = reader.capabilities().unwrap();
    assert_eq!(capabilities.version, "hermes 0.9.0");
    assert_eq!(capabilities.boards, ["pip-mdk", "pip-other"]);
    let tasks = reader.list_tasks("pip-mdk").unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(reader.show_task("pip-mdk", "task-1").unwrap(), tasks[0]);

    let commands = runner.commands.borrow();
    assert_eq!(commands.len(), 4);
    assert!(commands.iter().all(|command| command.program == "hermes"));
    for forbidden in ["add", "update", "archive", "complete", "create", "delete"] {
        assert!(
            commands
                .iter()
                .all(|command| !command.args.iter().any(|arg| arg == forbidden))
        );
    }
}

#[test]
fn malformed_oversized_failed_and_timed_out_commands_fail_closed() {
    let runner = FakeRunner::default();
    runner.output("not-json");
    assert!(matches!(
        reader(runner).list_tasks("pip-mdk"),
        Err(HermesError::MalformedJson(_))
    ));

    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(Ok(CommandOutput {
        status: 0,
        stdout: vec![b'x'; 4097],
        stderr: Vec::new(),
        timed_out: false,
    }));
    assert!(matches!(
        reader(runner).list_tasks("pip-mdk"),
        Err(HermesError::OutputTooLarge)
    ));

    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(Ok(CommandOutput {
        status: 2,
        stdout: Vec::new(),
        stderr: b"failed".to_vec(),
        timed_out: false,
    }));
    assert!(matches!(
        reader(runner).list_tasks("pip-mdk"),
        Err(HermesError::CommandFailed(2))
    ));

    let runner = FakeRunner::default();
    runner.outputs.borrow_mut().push_back(Ok(CommandOutput {
        status: 0,
        stdout: Vec::new(),
        stderr: Vec::new(),
        timed_out: true,
    }));
    assert!(matches!(
        reader(runner).list_tasks("pip-mdk"),
        Err(HermesError::TimedOut)
    ));
}

#[test]
fn projection_comparison_detects_missing_foreign_and_drifted_tasks() {
    let desired = vec![
        DesiredTask {
            projection_key: "plan-1".into(),
            title: "Plan".into(),
            assignee: "planner".into(),
            desired_status: "ready".into(),
        },
        DesiredTask {
            projection_key: "build-1".into(),
            title: "Build".into(),
            assignee: "builder".into(),
            desired_status: "blocked".into(),
        },
    ];
    let observed: Vec<pip_hermes::TaskSnapshot> = serde_json::from_str(
        r#"[
          {"id":"t1","title":"Wrong title","status":"ready","assignee":"planner","created_by":"pip-controller","body":"{\"projection_key\":\"plan-1\"}"},
          {"id":"foreign","title":"Other","status":"ready","assignee":"someone","created_by":"human","body":"{}"}
        ]"#,
    )
    .unwrap();

    assert_eq!(
        compare_projection(&desired, &observed),
        vec![
            Discrepancy::Drifted("plan-1".into()),
            Discrepancy::Missing("build-1".into()),
            Discrepancy::Foreign("foreign".into()),
        ]
    );
}

#[test]
fn process_runner_enforces_wall_clock_timeout_and_output_bound() {
    let runner = ProcessRunner;
    let timed_out = runner
        .run(&CommandSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "sleep 2".into()],
            timeout: Duration::from_millis(20),
            max_output_bytes: 1024,
        })
        .unwrap();
    assert!(timed_out.timed_out);

    let bounded = runner
        .run(&CommandSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "yes x | head -c 10000".into()],
            timeout: Duration::from_secs(2),
            max_output_bytes: 64,
        })
        .unwrap();
    assert!(bounded.stdout.len() <= 65);
}
