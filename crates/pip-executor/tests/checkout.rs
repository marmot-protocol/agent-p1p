use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::rc::Rc;
use std::time::Duration;

use pip_executor::{
    CheckoutError, CheckoutReconciler, ProcessError, ProcessOutput, ProcessRunner, ProcessSpec,
};

#[derive(Clone, Default)]
struct FakeRunner {
    outputs: Rc<RefCell<VecDeque<Result<ProcessOutput, ProcessError>>>>,
    commands: Rc<RefCell<Vec<ProcessSpec>>>,
}

impl FakeRunner {
    fn push(&self, status: i32, stdout: impl Into<Vec<u8>>) {
        self.outputs.borrow_mut().push_back(Ok(ProcessOutput {
            status,
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
fn exact_remote_default_branch_is_fetched_and_resolved_without_repo_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let runner = FakeRunner::default();
    runner.push(0, format!("{}\n", checkout.display()));
    runner.push(0, "https://github.com/marmot-protocol/mdk.git\n");
    runner.push(0, Vec::new());
    runner.push(0, format!("{}\n", "a".repeat(40)));
    runner.push(0, Vec::new());
    let reconciler = CheckoutReconciler::new(
        runner.clone(),
        "git",
        BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        Duration::from_secs(30),
        1_048_576,
    )
    .unwrap();

    assert_eq!(
        reconciler
            .fetch_default_head(
                &checkout,
                "https://github.com/marmot-protocol/mdk.git",
                "master",
            )
            .unwrap()
            .to_string(),
        "a".repeat(40)
    );
    let commands = runner.commands.borrow();
    assert_eq!(commands.len(), 5);
    assert!(commands.iter().all(|command| command.environment
        == BTreeMap::from([
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ])));
    let fetch = &commands[2].args;
    assert!(fetch.starts_with(&[
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "credential.helper=".into(),
    ]));
    assert!(fetch.windows(2).any(|pair| pair == ["fetch", "--no-tags"]));
    assert!(fetch.contains(&"https://github.com/marmot-protocol/mdk.git".into()));
}

#[test]
fn remote_url_drift_fails_before_fetch() {
    let temp = tempfile::tempdir().unwrap();
    let checkout = temp.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let runner = FakeRunner::default();
    runner.push(0, format!("{}\n", checkout.display()));
    runner.push(0, "https://attacker.invalid/repo.git\n");
    let reconciler = CheckoutReconciler::new(
        runner.clone(),
        "git",
        BTreeMap::new(),
        Duration::from_secs(30),
        1_048_576,
    )
    .unwrap();

    assert!(matches!(
        reconciler.fetch_default_head(
            &checkout,
            "https://github.com/marmot-protocol/mdk.git",
            "master",
        ),
        Err(CheckoutError::RemoteDrift)
    ));
    assert_eq!(runner.commands.borrow().len(), 2);
}

#[test]
fn worktree_verification_requires_exact_clean_branch_and_head() {
    let temp = tempfile::tempdir().unwrap();
    let worktree = temp.path().join("worktree");
    fs::create_dir(&worktree).unwrap();
    let runner = FakeRunner::default();
    runner.push(0, "pip/repo-1/issue-2/workflow-3\n");
    runner.push(0, format!("{}\n", "a".repeat(40)));
    runner.push(0, Vec::new());
    let reconciler = CheckoutReconciler::new(
        runner,
        "git",
        BTreeMap::new(),
        Duration::from_secs(30),
        1_048_576,
    )
    .unwrap();

    reconciler
        .verify_worktree(
            &worktree,
            "pip/repo-1/issue-2/workflow-3",
            "a".repeat(40).parse().unwrap(),
        )
        .unwrap();
}
