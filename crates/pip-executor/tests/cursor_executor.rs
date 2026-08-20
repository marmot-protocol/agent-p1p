use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::rc::Rc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use pip_contracts::{CaseIdentity, WorkerBinding, WorkerResult, WorkerRole};
use pip_executor::{
    CursorExecutionError, CursorExecutor, CursorTask, HealthAssurance, ProcessError, ProcessOutput,
    ProcessRunner, ProcessSpec, ProviderHealth,
};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<Result<ProcessOutput, ProcessError>>>>,
    commands: Rc<RefCell<Vec<ProcessSpec>>>,
}

impl FakeRunner {
    fn push(&self, stdout: Vec<u8>) {
        self.outputs.borrow_mut().push_back(Ok(ProcessOutput {
            status: 0,
            stdout,
            stderr: Vec::new(),
            timed_out: false,
        }));
    }
}

impl ProcessRunner for FakeRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
        self.commands.borrow_mut().push(spec.clone());
        self.outputs.borrow_mut().pop_front().unwrap()
    }
}

fn results() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

fn envelope(result: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": serde_json::to_string(result).unwrap(),
        "usage": {}
    }))
    .unwrap()
}

fn health(model: &str) -> ProviderHealth {
    ProviderHealth {
        provider: "cursor".into(),
        model: model.into(),
        version: "fixture-version".into(),
        assurance: HealthAssurance::AdvertisedExact,
    }
}

fn task(role: WorkerRole, model: &str, index: usize) -> CursorTask {
    let value = &results()[index];
    CursorTask {
        binding: WorkerBinding {
            case: CaseIdentity {
                repository_id: 984_321,
                issue_number: 1240,
                workflow_version: 1,
            },
            task_id: value["task_id"].as_str().unwrap().into(),
            role,
            requested_model: format!("cursor/{model}"),
            skills_repository_commit: "a".repeat(40),
            plan_version: 1,
            pr_number: value.get("pr_number").and_then(Value::as_u64),
            expected_head_sha: value
                .get("reviewed_head_sha")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        immutable_input: json!({"fixture": true, "plan_version": 1}),
        workflow_skill: "# Workflow Contract\nExact model and exact head.".into(),
        role_skill: "# Role Contract\nReturn structured evidence.".into(),
    }
}

fn executor(runner: FakeRunner) -> CursorExecutor<FakeRunner> {
    CursorExecutor::new(
        runner,
        "/opt/pip/bin/agent",
        "/usr/bin/git",
        BTreeMap::from([
            ("HOME".into(), "/var/lib/pip-provider".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]),
        Duration::from_secs(60),
        1_048_576,
    )
    .unwrap()
}

#[test]
fn builder_runs_once_in_fresh_exact_model_mode_and_retains_complete_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    let artifacts = tmp.path().join("artifacts");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    runner.push(envelope(&results()[1]));

    let result = executor(runner.clone())
        .execute(
            &health("composer-2.5"),
            &task(WorkerRole::Builder, "composer-2.5", 1),
            &worktree,
            &artifacts,
        )
        .unwrap();
    assert!(matches!(result, WorkerResult::Builder(_)));
    let command = &runner.commands.borrow()[0];
    assert_eq!(command.program, "/opt/pip/bin/agent");
    assert_eq!(command.cwd, worktree.canonicalize().unwrap());
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--model", "composer-2.5"])
    );
    assert!(command.args.iter().any(|arg| arg == "--force"));
    assert!(!command.args.iter().any(|arg| arg == "--resume"));
    let prompt = command.args.last().unwrap();
    assert!(prompt.contains("# Workflow Contract"));
    assert!(prompt.contains("# Role Contract"));
    assert!(prompt.contains("review-ready structured result contract"));
    assert_eq!(
        serde_json::from_str::<Value>(
            &fs::read_to_string(artifacts.join("run-status.json")).unwrap()
        )
        .unwrap(),
        json!({"status": "COMPLETE"})
    );
    assert!(artifacts.join("task-input.json").is_file());
    assert!(artifacts.join("prompt.md").is_file());
    assert!(artifacts.join("invocation.json").is_file());
    assert!(artifacts.join("model-verification.json").is_file());
    assert!(artifacts.join("stdout.log").is_file());
    assert!(artifacts.join("stderr.log").is_file());
    assert!(artifacts.join("result.json").is_file());
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&artifacts).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(&artifacts).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn reviewer_has_no_force_and_any_worktree_mutation_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    let artifacts = tmp.path().join("artifacts");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    runner.push(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n".to_vec());
    runner.push(Vec::new());
    runner.push(envelope(&results()[3]));
    runner.push(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n".to_vec());
    runner.push(b" M src/lib.rs\n".to_vec());

    assert!(matches!(
        executor(runner.clone()).execute(
            &health("claude-opus-4-8-thinking-high"),
            &task(
                WorkerRole::ReviewerSecperf,
                "claude-opus-4-8-thinking-high",
                3,
            ),
            &worktree,
            &artifacts,
        ),
        Err(CursorExecutionError::ReviewerMutation)
    ));
    let agent = &runner.commands.borrow()[2];
    assert!(!agent.args.iter().any(|arg| arg == "--force"));
    assert_eq!(
        serde_json::from_str::<Value>(
            &fs::read_to_string(artifacts.join("run-status.json")).unwrap()
        )
        .unwrap(),
        json!({"status": "INCOMPLETE"})
    );
    assert!(!artifacts.join("result.json").exists());
}

#[test]
fn model_or_task_binding_mismatch_is_never_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    assert!(matches!(
        executor(runner.clone()).execute(
            &health("auto"),
            &task(WorkerRole::Builder, "composer-2.5", 1),
            &worktree,
            &tmp.path().join("artifacts-1"),
        ),
        Err(CursorExecutionError::HealthBindingMismatch)
    ));
    assert!(runner.commands.borrow().is_empty());

    let mut wrong = results()[1].clone();
    wrong["task_id"] = json!("another-task");
    runner.push(envelope(&wrong));
    assert!(matches!(
        executor(runner).execute(
            &health("composer-2.5"),
            &task(WorkerRole::Builder, "composer-2.5", 1),
            &worktree,
            &tmp.path().join("artifacts-2"),
        ),
        Err(CursorExecutionError::InvalidResult(_))
    ));
}

#[test]
fn secret_input_is_rejected_before_artifacts_and_timeout_remains_incomplete() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    let mut unsafe_task = task(WorkerRole::Builder, "composer-2.5", 1);
    unsafe_task.immutable_input = json!({"canary": format!("ghp_{}", "a".repeat(30))});
    let unsafe_artifacts = tmp.path().join("unsafe-artifacts");
    assert!(matches!(
        executor(runner.clone()).execute(
            &health("composer-2.5"),
            &unsafe_task,
            &worktree,
            &unsafe_artifacts,
        ),
        Err(CursorExecutionError::UnsafeSecretInput)
    ));
    assert!(!unsafe_artifacts.exists());

    runner.outputs.borrow_mut().push_back(Ok(ProcessOutput {
        status: -1,
        stdout: b"partial".to_vec(),
        stderr: Vec::new(),
        timed_out: true,
    }));
    let timeout_artifacts = tmp.path().join("timeout-artifacts");
    assert!(matches!(
        executor(runner).execute(
            &health("composer-2.5"),
            &task(WorkerRole::Builder, "composer-2.5", 1),
            &worktree,
            &timeout_artifacts,
        ),
        Err(CursorExecutionError::TimedOut)
    ));
    assert_eq!(
        serde_json::from_str::<Value>(
            &fs::read_to_string(timeout_artifacts.join("run-status.json")).unwrap()
        )
        .unwrap(),
        json!({"status": "INCOMPLETE"})
    );
}
