use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::rc::Rc;

use pip_contracts::{WorkerResult, WorkerRole};
use pip_control::{CursorDirectRuntime, DirectWorkerRuntime};
use pip_controller::DirectTaskSpec;
use pip_executor::{ProcessError, ProcessOutput, ProcessRunner, ProcessSpec};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<Result<ProcessOutput, ProcessError>>>>,
    commands: Rc<RefCell<Vec<ProcessSpec>>>,
}

impl FakeRunner {
    fn push(&self, stdout: impl Into<Vec<u8>>) {
        self.outputs.borrow_mut().push_back(Ok(ProcessOutput {
            status: 0,
            stdout: stdout.into(),
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

#[test]
fn production_runtime_probes_exact_model_reads_canonical_skills_and_retains_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let worktrees = temp.path().join("worktrees");
    let worktree = worktrees.join("repo-1055628515-issue-1240-workflow-2");
    let artifacts = temp.path().join("artifacts");
    let skills = temp.path().join("skills");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir(&artifacts).unwrap();
    fs::create_dir_all(skills.join("shared/workflow-contract")).unwrap();
    fs::create_dir_all(skills.join("builder-grok")).unwrap();
    fs::write(
        skills.join("shared/workflow-contract/SKILL.md"),
        "# Workflow contract\nReturn the exact bound result.\n",
    )
    .unwrap();
    fs::write(
        skills.join("builder-grok/SKILL.md"),
        "# Builder\nBuild only in the assigned worktree.\n",
    )
    .unwrap();

    let runner = FakeRunner::default();
    runner.push("cursor-agent 1.2.3\n");
    runner.push("models    List available models\n");
    runner.push("Authenticated as fixture@example.com\n");
    runner.push("composer-2.5\n");
    runner.push(envelope(&builder_result()));
    let runtime = CursorDirectRuntime::new(
        runner.clone(),
        "cursor-agent",
        "git",
        &worktrees,
        &artifacts,
        &skills,
        BTreeMap::from([
            (
                "HOME".into(),
                temp.path().join("provider-home").display().to_string(),
            ),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]),
        1_048_576,
    )
    .unwrap();

    assert!(matches!(
        runtime.execute(&direct_task(&worktree), 7).unwrap(),
        WorkerResult::Builder(_)
    ));
    let commands = runner.commands.borrow();
    assert_eq!(commands.len(), 5);
    assert_eq!(commands[3].args, ["models"]);
    assert!(
        commands[4]
            .args
            .windows(2)
            .any(|pair| pair == ["--model", "composer-2.5"])
    );
    let canonical_worktree = worktree.canonicalize().unwrap();
    assert!(
        commands
            .iter()
            .all(|command| command.cwd == canonical_worktree)
    );
    drop(commands);

    let task_roots = fs::read_dir(&artifacts)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(task_roots.len(), 1);
    let attempts = fs::read_dir(task_roots[0].path())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].file_name(), "attempt-00007");
    assert!(attempts[0].path().join("result.json").is_file());
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(attempts[0].path().join("run-status.json")).unwrap()
        )
        .unwrap()["status"],
        "COMPLETE"
    );
}

fn direct_task(worktree: &std::path::Path) -> DirectTaskSpec {
    DirectTaskSpec {
        schema_version: 1,
        source_effect_id: "effect-dispatch-builder".into(),
        task_id: task_id().into(),
        title: "Run builder".into(),
        body: json!({
            "case_key": "repo:1055628515#1240@2",
            "repository_id": 1_055_628_515_u64,
            "issue_number": 1240,
            "workflow_version": 2,
            "state_revision": 2,
            "role": "builder",
            "remediation_round": 1,
            "plan_version": 1,
            "assigned_branch": "pip/repo-1055628515/issue-1240/workflow-2",
            "assigned_worktree": worktree,
            "execution": "direct",
            "provider": "cursor",
            "model": "composer-2.5",
            "requested_model": "cursor/composer-2.5",
            "skills_repository_commit": "a".repeat(40),
            "immutable_evidence_bundle": {"schema_version": 1, "sha256": "b".repeat(64)},
        }),
        role: WorkerRole::Builder,
        profile: "builder-grok".into(),
        workspace: worktree.display().to_string(),
        skills: vec!["workflow-contract".into(), "builder-grok".into()],
        provider: "cursor".into(),
        model: "composer-2.5".into(),
        max_runtime: "PT45M".into(),
        priority: 50,
    }
}

fn builder_result() -> Value {
    let mut value = serde_json::from_str::<Value>(include_str!(
        "../../../migration/target-v1/worker-results.json"
    ))
    .unwrap()["results"][1]
        .clone();
    value["workflow_version"] = json!(2);
    value["case"]["repository_id"] = json!(1_055_628_515_u64);
    value["case"]["workflow_version"] = json!(2);
    value["task_id"] = json!(task_id());
    value
}

fn envelope(result: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": serde_json::to_string(result).unwrap(),
    }))
    .unwrap()
}

fn task_id() -> &'static str {
    "repo:1055628515#1240@2:builder:round:1:revision:2:worker"
}
