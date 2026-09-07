use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::rc::Rc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use pip_contracts::{CaseIdentity, ReviewMode, WorkerBinding, WorkerResult, WorkerRole};
use pip_executor::{
    CursorExecutionError, CursorExecutor, CursorTask, HealthAssurance, ProcessError, ProcessOutput,
    ProcessRunner, ProcessSpec, ProviderHealth,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[test]
fn direct_prompt_uses_digest_bound_evidence_without_repeating_history() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let artifacts = tmp.path().join("artifacts");
    let runner = FakeRunner::default();
    runner.push(envelope(&results()[1]));
    let mut input = task(WorkerRole::Builder, "composer-2.5", 1);
    let bundle = json!({"records": "x".repeat(200_000)});
    input.immutable_input["immutable_evidence_bundle"] = bundle.clone();
    executor(runner)
        .execute(&health("composer-2.5"), &input, &worktree, &artifacts)
        .unwrap();
    assert!(fs::read(artifacts.join("prompt.md")).unwrap().len() < 4096);
    let bytes = fs::read(artifacts.join("immutable-evidence.json")).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), bundle);
    let task_input: Value =
        serde_json::from_slice(&fs::read(artifacts.join("task-input.json")).unwrap()).unwrap();
    assert!(
        task_input["input"]
            .get("immutable_evidence_bundle")
            .is_none()
    );
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        task_input["input"]["immutable_evidence_ref"],
        json!({"schema_version":1,"path":artifacts.join("immutable-evidence.json"),"sha256":digest})
    );
}

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

#[test]
fn cursor_progress_text_may_surround_one_bound_result_but_not_two() {
    let result = serde_json::to_string(&results()[1]).unwrap();
    for (text, accepted) in [
        (
            format!(
                "Checking source.{{\"progress\":\"compiled\"}}```json\n{result}\n```Tests finished."
            ),
            true,
        ),
        (format!("{result}Next reply: {result}"), false),
        ("No structured result was returned.".into(), false),
        (
            format!("{{\"contract_version\":2,\"role\":\"builder\"}}{result}"),
            false,
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("worktree");
        fs::create_dir(&worktree).unwrap();
        let runner = FakeRunner::default();
        runner.push(
            serde_json::to_vec(&json!({
                "type":"result", "subtype":"success", "is_error":false, "result":text
            }))
            .unwrap(),
        );
        let observed = executor(runner).execute(
            &health("composer-2.5"),
            &task(WorkerRole::Builder, "composer-2.5", 1),
            &worktree,
            &tmp.path().join("artifacts"),
        );
        assert_eq!(observed.is_ok(), accepted, "{observed:?}");
    }
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
            reviewer_id: value
                .get("reviewer_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            review_mode: value
                .get("reviewer_id")
                .is_some()
                .then_some(ReviewMode::Required),
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
fn large_prompt_does_not_depend_on_operating_system_argument_size() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    runner.push(envelope(&results()[1]));
    let mut input = task(WorkerRole::Builder, "composer-2.5", 1);
    input.workflow_skill = "x".repeat(256 * 1024);
    executor(runner.clone())
        .execute(
            &health("composer-2.5"),
            &input,
            &worktree,
            &tmp.path().join("artifacts"),
        )
        .unwrap();
    let commands = runner.commands.borrow();
    assert!(commands[0].args.iter().all(|arg| arg.len() < 4096));
    assert!(
        fs::metadata(tmp.path().join("artifacts/prompt.md"))
            .unwrap()
            .len()
            > 256 * 1024
    );
}

#[test]
fn frontmatter_prompt_is_input_data_not_a_cli_option() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    runner.push(envelope(&results()[1]));
    let mut input = task(WorkerRole::Builder, "composer-2.5", 1);
    input.workflow_skill = "---\nname: workflow-contract\n---\n# Contract".into();
    executor(runner.clone())
        .execute(
            &health("composer-2.5"),
            &input,
            &worktree,
            &tmp.path().join("artifacts"),
        )
        .unwrap();
    let commands = runner.commands.borrow();
    let args = &commands[0].args;
    assert!(!args.iter().any(|arg| arg.starts_with("---\n")));
    assert!(
        fs::read_to_string(commands[0].stdin_file.as_ref().unwrap())
            .unwrap()
            .starts_with("---\n")
    );
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
    assert_eq!(command.environment["GIT_CONFIG_KEY_0"], "safe.directory");
    assert_eq!(
        command.environment["GIT_CONFIG_VALUE_0"],
        worktree.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(
        command
            .args
            .windows(2)
            .any(|pair| pair == ["--model", "composer-2.5"])
    );
    assert!(command.args.iter().any(|arg| arg == "--force"));
    assert!(!command.args.iter().any(|arg| arg == "--resume"));
    let prompt = fs::read_to_string(command.stdin_file.as_ref().unwrap()).unwrap();
    assert!(prompt.contains("# Workflow Contract"));
    assert!(prompt.contains("# Role Contract"));
    assert!(prompt.contains("review-ready structured result contract"));
    assert!(prompt.contains(artifacts.to_str().unwrap()));
    assert!(prompt.contains("worker-result.json"));
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
    let invocation: Value =
        serde_json::from_slice(&fs::read(artifacts.join("invocation.json")).unwrap()).unwrap();
    assert_eq!(
        invocation["environment_keys"],
        json!(command.environment.keys().collect::<Vec<_>>())
    );
    assert!(artifacts.join("model-verification.json").is_file());
    assert!(artifacts.join("stdout.log").is_file());
    assert!(artifacts.join("stderr.log").is_file());
    assert!(artifacts.join("result.json").is_file());
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&artifacts).unwrap().permissions().mode() & 0o7777,
            0o750
        );
        for entry in fs::read_dir(&artifacts).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o640
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
    assert!(agent.args.iter().any(|arg| arg == "--trust"));
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
    unsafe_task.immutable_input =
        json!({"immutable_evidence_bundle": {"canary": format!("ghp_{}", "a".repeat(30))}});
    assert!(matches!(
        executor(runner.clone()).execute(
            &health("composer-2.5"),
            &unsafe_task,
            &worktree,
            &unsafe_artifacts
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
