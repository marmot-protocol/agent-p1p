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
    bootstrap(false);
}

#[test]
fn conversation_opt_in_uses_the_configured_reply_model_and_reasoning() {
    bootstrap(true);
}

#[test]
fn conversation_model_is_independent_of_the_planner() {
    let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    value["conversation_model"] = serde_json::json!({
        "provider":"openai-codex", "model":"gpt-6.1-sol", "reasoning_effort":"high"
    });
    let policy = load_repository_policy(&serde_json::to_vec(&value).unwrap()).unwrap();
    let binding = policy.conversation_binding().unwrap();
    assert_eq!(binding.model, "gpt-6.1-sol");
    assert_eq!(binding.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(policy.roles[0].model, "gpt-6-astra");
}

fn bootstrap(conversations: bool) {
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
        "conversation",
    ] {
        fs::create_dir_all(skills.join(path)).unwrap();
        fs::write(skills.join(path).join("SKILL.md"), "# managed\n").unwrap();
    }
    let runner = FakeRunner::default();
    runner.output("hermes 0.9.0\n");
    runner.output(r#"[{"slug":"pip-mdk","name":"Pip - marmot-protocol/mdk"}]"#);
    runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    runner.output("gateway run --external-supervisor\n");
    let mut profiles = vec![
        ("gpt-6-astra", "xhigh"),
        ("gpt-6.1-sol", "high"),
        ("gpt-6-astra", "xhigh"),
    ];
    if conversations {
        profiles.push(("gpt-6.1-sol", "high"));
    }
    for (model, reasoning) in profiles {
        runner.output(&format!(
            r#"{{"default":"{model}","provider":"openai-codex"}}"#
        ));
        runner.output(&format!("\"{reasoning}\"\n"));
        runner.output("\"profile\"\n");
    }
    let mut policy = load_repository_policy(include_bytes!(
        "../../../config/target/repositories/mdk.json"
    ))
    .unwrap();
    policy.conversations_enabled = conversations;

    let outcome = bootstrap_hermes_runtime_with(
        &policy,
        &root,
        &skills,
        &root.join("auth.json"),
        "hermes",
        runner,
    )
    .unwrap();
    assert_eq!(outcome.profiles_created, if conversations { 4 } else { 3 });
    assert_eq!(root.join("profiles/conversation").is_dir(), conversations);
    if conversations {
        let reply: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("profiles/conversation/config.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(reply["model"], "gpt-6.1-sol");
        assert_eq!(reply["agent"]["reasoning_effort"], "high");
    }
    assert!(root.join("profiles/planner").is_dir());
    assert!(root.join("profiles/reviewer-general").is_dir());
    assert!(root.join("profiles/final-reviewer").is_dir());
    assert!(!root.join("profiles/builder").exists());
    assert!(!root.join("profiles/reviewer-secperf").exists());
    let planner: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("profiles/planner/config.yaml")).unwrap())
            .unwrap();
    assert_eq!(planner["agent"]["reasoning_effort"], "xhigh");
    let root_config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("config.yaml")).unwrap()).unwrap();
    assert_eq!(root_config["kanban"]["max_in_progress"], 2);
    assert_eq!(root_config["kanban"]["max_in_progress_per_profile"], 1);
}
