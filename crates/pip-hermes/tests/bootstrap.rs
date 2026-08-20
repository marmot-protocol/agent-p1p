use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::rc::Rc;
use std::time::Duration;

use pip_hermes::{
    CommandOutput, CommandRunner, CommandSpec, HermesBootstrap, HermesError, ProfileBootstrapSpec,
    RuntimeBootstrapSpec,
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

#[test]
fn bootstrap_creates_only_managed_profiles_and_reprobes_the_board() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("hermes");
    let skills = temp.path().join("skills");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(root.join("auth.json"), "{\"credential_pool\":{}}\n").unwrap();
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
    runner.output("[]");
    runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    runner.output("gateway run --no-supervise\n");
    runner.output("Board 'pip-mdk' created.\n");
    runner.output("hermes 0.9.0\n");
    runner.output(r#"[{"name":"pip-mdk"}]"#);
    profile_outputs(&runner);
    let bootstrap = HermesBootstrap::new(
        runner.clone(),
        "hermes",
        Duration::from_secs(5),
        1024 * 1024,
    )
    .unwrap();

    let outcome = bootstrap.apply(&spec(&root, &skills)).unwrap();
    assert!(outcome.board_created);
    assert_eq!(outcome.profiles_created, 3);
    assert_eq!(outcome.profiles_reconciled, 0);
    let commands = runner.commands.borrow();
    assert_eq!(commands.len(), 19);
    assert_eq!(
        commands[4].args,
        [
            "kanban",
            "boards",
            "create",
            "pip-mdk",
            "--name",
            "Pip v2 - marmot-protocol/mdk",
            "--description",
            "Pip v2 controlled shadow workflow for marmot-protocol/mdk",
        ]
    );
    drop(commands);

    let root_config: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("config.yaml")).unwrap()).unwrap();
    assert_eq!(root_config["kanban"]["dispatch_in_gateway"], true);
    assert_eq!(root_config["kanban"]["max_in_progress"], 1);
    assert_eq!(root_config["kanban"]["auto_decompose"], false);
    let planner = root.join("profiles/planner");
    let planner_config: serde_json::Value =
        serde_json::from_slice(&fs::read(planner.join("config.yaml")).unwrap()).unwrap();
    assert_eq!(planner_config["provider"], "openai-codex");
    assert_eq!(planner_config["model"], "gpt-5.6-sol");
    assert_eq!(planner_config["agent"]["reasoning_effort"], "xhigh");
    assert_eq!(planner_config["max_concurrent_sessions"], 1);
    assert_eq!(planner_config["terminal"]["home_mode"], "profile");
    assert_eq!(
        planner_config["platform_toolsets"]["cli"],
        serde_json::json!(["file", "terminal"])
    );
    assert_eq!(planner_config["kanban"]["dispatch_in_gateway"], false);
    assert_eq!(
        fs::read_link(planner.join("auth.json")).unwrap(),
        root.join("auth.json").canonicalize().unwrap()
    );
    assert_eq!(
        fs::read_link(planner.join("skills/planner")).unwrap(),
        skills.join("planner").canonicalize().unwrap()
    );
    assert_eq!(
        fs::read_link(planner.join("skills/workflow-contract")).unwrap(),
        skills
            .join("shared/workflow-contract")
            .canonicalize()
            .unwrap()
    );

    let retry_runner = FakeRunner::default();
    retry_runner.output("hermes 0.9.0\n");
    retry_runner.output(r#"[{"name":"pip-mdk"}]"#);
    retry_runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    retry_runner.output("gateway run --no-supervise\n");
    profile_outputs(&retry_runner);
    let retry = HermesBootstrap::new(
        retry_runner.clone(),
        "hermes",
        Duration::from_secs(5),
        1024 * 1024,
    )
    .unwrap()
    .apply(&spec(&root, &skills))
    .unwrap();
    assert!(!retry.board_created);
    assert_eq!(retry.profiles_created, 0);
    assert_eq!(retry.profiles_reconciled, 3);
    assert_eq!(retry_runner.commands.borrow().len(), 16);
}

#[test]
fn bootstrap_refuses_unmanaged_profile_or_redirected_authentication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("hermes");
    let skills = temp.path().join("skills");
    fs::create_dir_all(root.join("profiles/planner")).unwrap();
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
    symlink(
        temp.path().join("elsewhere.json"),
        root.join("profiles/planner/auth.json"),
    )
    .unwrap();
    let runner = FakeRunner::default();
    runner.output("hermes 0.9.0\n");
    runner.output(r#"[{"name":"pip-mdk"}]"#);
    runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    runner.output("gateway run --no-supervise\n");
    let bootstrap = HermesBootstrap::new(runner, "hermes", Duration::from_secs(5), 4096).unwrap();

    assert!(bootstrap.apply(&spec(&root, &skills)).is_err());
    assert!(!root.join("profiles/planner/.pip-v2-profile.json").exists());
}

fn spec(root: &std::path::Path, skills: &std::path::Path) -> RuntimeBootstrapSpec {
    RuntimeBootstrapSpec {
        root: root.into(),
        skills_root: skills.into(),
        auth_source: root.join("auth.json"),
        board: "pip-mdk".into(),
        board_name: "Pip v2 - marmot-protocol/mdk".into(),
        board_description: "Pip v2 controlled shadow workflow for marmot-protocol/mdk".into(),
        profiles: vec![
            profile("planner", "xhigh"),
            profile("reviewer-general", "high"),
            profile("final-reviewer", "xhigh"),
        ],
    }
}

fn profile(name: &str, reasoning: &str) -> ProfileBootstrapSpec {
    ProfileBootstrapSpec {
        name: name.into(),
        provider: "openai-codex".into(),
        model: "gpt-5.6-sol".into(),
        reasoning_effort: reasoning.into(),
        skills: vec!["workflow-contract".into(), name.into()],
    }
}

fn profile_outputs(runner: &FakeRunner) {
    for reasoning in ["xhigh", "high", "xhigh"] {
        runner.output("gpt-5.6-sol\n");
        runner.output("openai-codex\n");
        runner.output(&format!("{reasoning}\n"));
        runner.output("profile\n");
    }
}
