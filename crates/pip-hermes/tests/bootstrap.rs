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
    runner.output("gateway run --external-supervisor\n");
    runner.output("Board 'pip-mdk' created.\n");
    runner.output("hermes 0.9.0\n");
    runner.output(r#"[{"slug":"pip-mdk","name":"Pip - marmot-protocol/mdk"}]"#);
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
    assert_eq!(commands.len(), 16);
    assert_eq!(
        commands[4].args,
        [
            "kanban",
            "boards",
            "create",
            "pip-mdk",
            "--name",
            "Pip - marmot-protocol/mdk",
            "--description",
            "Pip controlled shadow workflow for marmot-protocol/mdk",
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
    assert_eq!(planner_config["lsp"]["enabled"], false);
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

    // Stock Hermes seeds these on the first worker run. They are not Pip-owned.
    fs::write(
        planner.join("skills/.bundled_manifest"),
        "{\"fixture\":true}\n",
    )
    .unwrap();
    fs::create_dir_all(planner.join("skills/autonomous-ai-agents/hermes-agent")).unwrap();
    fs::write(
        planner.join("skills/autonomous-ai-agents/hermes-agent/SKILL.md"),
        "# upstream runtime\n",
    )
    .unwrap();
    fs::create_dir_all(planner.join("skills/user-notes")).unwrap();
    fs::write(
        planner.join("skills/user-notes/SKILL.md"),
        "# Preserve me\n",
    )
    .unwrap();
    let retry_runner = FakeRunner::default();
    retry_runner.output("hermes 0.9.0\n");
    retry_runner.output(r#"[{"slug":"pip-mdk","name":"Pip - marmot-protocol/mdk"}]"#);
    retry_runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    retry_runner.output("gateway run --external-supervisor\n");
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
    assert_eq!(retry_runner.commands.borrow().len(), 13);
    assert_eq!(
        fs::read_to_string(planner.join("skills/.bundled_manifest")).unwrap(),
        "{\"fixture\":true}\n"
    );
    assert_eq!(
        fs::read_to_string(planner.join("skills/user-notes/SKILL.md")).unwrap(),
        "# Preserve me\n"
    );
    assert_eq!(
        fs::read_to_string(planner.join("skills/autonomous-ai-agents/hermes-agent/SKILL.md"))
            .unwrap(),
        "# upstream runtime\n"
    );
    // A collision at an owned link is still refused before any CLI or rewrite.
    fs::remove_file(planner.join("skills/planner")).unwrap();
    fs::create_dir(planner.join("skills/planner")).unwrap();
    fs::write(
        planner.join("skills/planner/SKILL.md"),
        "# foreign collision\n",
    )
    .unwrap();
    let untouched = fs::read(planner.join("config.yaml")).unwrap();
    let blocked = FakeRunner::default();
    assert!(
        HermesBootstrap::new(blocked.clone(), "hermes", Duration::from_secs(5), 4096)
            .unwrap()
            .apply(&spec(&root, &skills))
            .is_err()
    );
    assert!(blocked.commands.borrow().is_empty());
    assert_eq!(fs::read(planner.join("config.yaml")).unwrap(), untouched);
    assert_eq!(
        fs::read_to_string(planner.join("skills/planner/SKILL.md")).unwrap(),
        "# foreign collision\n"
    );
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
    runner.output(r#"[{"slug":"pip-mdk","name":"Pip - marmot-protocol/mdk"}]"#);
    runner.output("--workspace --idempotency-key --created-by --max-runtime --max-retries --skill --model --provider --initial-status\n");
    runner.output("gateway run --external-supervisor\n");
    let bootstrap = HermesBootstrap::new(runner, "hermes", Duration::from_secs(5), 4096).unwrap();

    assert!(bootstrap.apply(&spec(&root, &skills)).is_err());
    assert!(!root.join("profiles/planner/.pip-profile.json").exists());
}

fn spec(root: &std::path::Path, skills: &std::path::Path) -> RuntimeBootstrapSpec {
    RuntimeBootstrapSpec {
        root: root.into(),
        skills_root: skills.into(),
        auth_source: root.join("auth.json"),
        board: "pip-mdk".into(),
        board_name: "Pip - marmot-protocol/mdk".into(),
        board_description: "Pip controlled shadow workflow for marmot-protocol/mdk".into(),
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
        runner.output("default: gpt-5.6-sol\nprovider: openai-codex\n");
        runner.output(&format!("{reasoning}\n"));
        runner.output("profile\n");
    }
}

/// No provider credentials or workers: use stock Hermes's real skill sync and
/// then reconcile the same profiles twice, including a new release skill root.
#[test]
#[ignore = "requires PIP_TEST_HERMES and PIP_TEST_SKILLS_ROOT; no provider is called"]
fn stock_hermes_skill_sync_survives_rebootstrap_and_release_relink() {
    use pip_hermes::ProcessRunner;
    let hermes = std::env::var("PIP_TEST_HERMES").unwrap();
    let skills = std::path::PathBuf::from(std::env::var("PIP_TEST_SKILLS_ROOT").unwrap());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("hermes");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("auth.json"), "{}\n").unwrap();
    fs::set_permissions(root.join("auth.json"), fs::Permissions::from_mode(0o600)).unwrap();
    let runner = ProcessRunner::for_hermes_root(&root).unwrap();
    let bootstrap = HermesBootstrap::new(
        runner.clone(),
        &hermes,
        Duration::from_secs(30),
        1024 * 1024,
    )
    .unwrap();
    let mut spec = spec(&root, &skills);
    spec.board = "pip-isolated-bootstrap-proof".into();
    let first = bootstrap.apply(&spec).unwrap();
    assert_eq!(first.profiles_created, 3);
    let sync = runner
        .run(&CommandSpec {
            program: hermes,
            args: ["-p", "planner", "skills", "opt-in", "--sync"]
                .map(String::from)
                .to_vec(),
            timeout: Duration::from_secs(45),
            max_output_bytes: 1024 * 1024,
        })
        .unwrap();
    assert_eq!(sync.status, 0, "{}", String::from_utf8_lossy(&sync.stderr));
    assert!(!sync.timed_out);
    let profile = root.join("profiles/planner");
    let manifest_path = profile.join("skills/.bundled_manifest");
    let manifest = fs::read(&manifest_path).unwrap();
    assert!(!manifest.is_empty());
    let second = bootstrap.apply(&spec).unwrap();
    assert_eq!(second.profiles_reconciled, 3);
    assert_eq!(fs::read(&manifest_path).unwrap(), manifest);
    let reference = profile.join("skills/workflow-contract/references/worker-result-contracts.md");
    let guide = fs::read_to_string(&reference).unwrap();
    assert!(guide.contains("## Planner"));
    assert!(guide.contains("kanban_complete"));

    let next_skills = temp.path().join("next-release-skills");
    for relative in [
        "shared/workflow-contract",
        "planner",
        "reviewer-general",
        "final-reviewer",
    ] {
        fs::create_dir_all(next_skills.join(relative)).unwrap();
        fs::copy(
            skills.join(relative).join("SKILL.md"),
            next_skills.join(relative).join("SKILL.md"),
        )
        .unwrap();
    }
    fs::create_dir_all(next_skills.join("shared/workflow-contract/references")).unwrap();
    fs::write(
        next_skills.join("shared/workflow-contract/references/worker-result-contracts.md"),
        &guide,
    )
    .unwrap();
    spec.skills_root = next_skills.clone();
    let upgraded = bootstrap.apply(&spec).unwrap();
    assert_eq!(upgraded.profiles_reconciled, 3);
    assert_eq!(fs::read(&manifest_path).unwrap(), manifest);
    assert_eq!(
        fs::read_link(profile.join("skills/workflow-contract")).unwrap(),
        next_skills
            .join("shared/workflow-contract")
            .canonicalize()
            .unwrap()
    );
    assert_eq!(fs::read_to_string(reference).unwrap(), guide);
    println!("STOCK_HERMES_REBOOTSTRAP_AND_RELEASE_RELINK_OK");
}
