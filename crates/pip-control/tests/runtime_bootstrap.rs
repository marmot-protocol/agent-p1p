use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

use pip_control::{bootstrap_hermes_runtime_with, load_repository_policy};
use pip_hermes::{CommandOutput, CommandRunner, CommandSpec, HermesError};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<Result<CommandOutput, HermesError>>>>,
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
    fn run(&self, _spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        self.outputs.borrow_mut().pop_front().unwrap()
    }
}

#[test]
fn policy_bootstrap_manages_only_hermes_roles_with_exact_reasoning() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("hermes");
    let skills = temp.path().join("skills");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("auth.json"), "{}\n").unwrap();
    fs::set_permissions(root.join("auth.json"), fs::Permissions::from_mode(0o600)).unwrap();
    for path in [
        "shared/workflow-contract",
        "planner",
        "reviewer-general",
        "final-reviewer",
    ] {
        fs::create_dir_all(skills.join(path)).unwrap();
        fs::write(skills.join(path).join("SKILL.md"), "# managed\n").unwrap();
    }
    let runner = FakeRunner::default();
    runner.output("hermes 0.9.0\n");
    runner.output(r#"[{"name":"pip-mdk"}]"#);
    runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    runner.output("gateway run --no-supervise\n");
    for reasoning in ["xhigh", "high", "xhigh"] {
        runner.output("gpt-5.6-sol\n");
        runner.output("openai-codex\n");
        runner.output(&format!("{reasoning}\n"));
        runner.output("profile\n");
    }
    let policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();

    let outcome = bootstrap_hermes_runtime_with(
        &policy,
        &root,
        &skills,
        &root.join("auth.json"),
        "hermes",
        runner,
    )
    .unwrap();
    assert_eq!(outcome.profiles_created, 3);
    assert!(root.join("profiles/planner").is_dir());
    assert!(root.join("profiles/reviewer-general").is_dir());
    assert!(root.join("profiles/final-reviewer").is_dir());
    assert!(!root.join("profiles/builder-grok").exists());
    assert!(!root.join("profiles/reviewer-secperf").exists());
    let planner: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("profiles/planner/config.yaml")).unwrap())
            .unwrap();
    assert_eq!(planner["agent"]["reasoning_effort"], "xhigh");
}
