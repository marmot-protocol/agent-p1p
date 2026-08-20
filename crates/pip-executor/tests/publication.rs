use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use pip_core::GitSha;
use pip_executor::{
    AllocationError, GitCommand, GitOutput, GitPublicationSpec, GitPublisher, GitRunner,
    ProcessGitRunner, PublicationError, PublicationResult,
};

#[derive(Clone, Default)]
struct FakeGit {
    outputs: Rc<RefCell<VecDeque<GitOutput>>>,
    commands: Rc<RefCell<Vec<GitCommand>>>,
}

impl FakeGit {
    fn push(&self, status: i32, stdout: impl Into<Vec<u8>>) {
        self.outputs.borrow_mut().push_back(GitOutput {
            status,
            stdout: stdout.into(),
            stderr: Vec::new(),
            timed_out: false,
        });
    }
}

impl GitRunner for FakeGit {
    fn run(&self, command: &GitCommand) -> Result<GitOutput, AllocationError> {
        self.commands.borrow_mut().push(command.clone());
        Ok(self.outputs.borrow_mut().pop_front().unwrap())
    }
}

fn sha(byte: char) -> GitSha {
    GitSha::from_str(&byte.to_string().repeat(40)).unwrap()
}

fn spec(worktree: &Path, remote: Option<GitSha>) -> GitPublicationSpec {
    GitPublicationSpec::new(
        worktree,
        "origin",
        "pip/v2/repo-984321/issue-1240/workflow-2",
        sha('b'),
        remote,
    )
    .unwrap()
}

fn publisher(runner: FakeGit) -> GitPublisher<FakeGit> {
    GitPublisher::new(runner, "git", Duration::from_secs(5), 16_384).unwrap()
}

#[test]
fn scoped_publication_rejects_a_worktree_outside_the_controller_root() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("owned");
    let inside = root.join("repo-984321-issue-1240-workflow-2");
    let outside = tmp.path().join("foreign");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&inside).unwrap();
    std::fs::create_dir(&outside).unwrap();

    assert!(
        GitPublicationSpec::new_scoped(
            &root,
            &inside,
            "origin",
            "https://github.com/marmot-protocol/mdk.git",
            "pip/v2/repo-984321/issue-1240/workflow-2",
            sha('b'),
            None,
        )
        .is_ok()
    );
    assert!(matches!(
        GitPublicationSpec::new_scoped(
            &root,
            &outside,
            "origin",
            "https://github.com/marmot-protocol/mdk.git",
            "pip/v2/repo-984321/issue-1240/workflow-2",
            sha('b'),
            None,
        ),
        Err(PublicationError::InvalidSpec)
    ));
}

#[test]
fn scoped_publication_rejects_remote_url_drift_before_push() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("owned");
    let worktree = root.join("repo-984321-issue-1240-workflow-2");
    std::fs::create_dir_all(&worktree).unwrap();
    let spec = GitPublicationSpec::new_scoped(
        &root,
        &worktree,
        "origin",
        "https://github.com/marmot-protocol/mdk.git",
        "pip/v2/repo-984321/issue-1240/workflow-2",
        sha('b'),
        None,
    )
    .unwrap();
    let runner = FakeGit::default();
    runner.push(0, b"https://attacker.invalid/foreign.git\n".to_vec());

    assert!(matches!(
        publisher(runner.clone()).publish(&spec),
        Err(PublicationError::RemoteUrlDrift)
    ));
    assert!(
        !runner
            .commands
            .borrow()
            .iter()
            .any(|command| command.args.iter().any(|argument| argument == "push"))
    );
}

#[test]
fn scoped_publication_rejects_multiple_push_urls_before_network_access() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("owned");
    let worktree = root.join("repo-984321-issue-1240-workflow-2");
    std::fs::create_dir_all(&worktree).unwrap();
    let spec = GitPublicationSpec::new_scoped(
        &root,
        &worktree,
        "origin",
        "https://github.com/marmot-protocol/mdk.git",
        "pip/v2/repo-984321/issue-1240/workflow-2",
        sha('b'),
        None,
    )
    .unwrap();
    let runner = FakeGit::default();
    runner.push(
        0,
        b"https://github.com/marmot-protocol/mdk.git\nhttps://attacker.invalid/copy.git\n".to_vec(),
    );

    assert!(matches!(
        publisher(runner.clone()).publish(&spec),
        Err(PublicationError::RemoteUrlDrift)
    ));
    let commands = runner.commands.borrow();
    assert_eq!(
        commands[0]
            .args
            .iter()
            .rev()
            .take(5)
            .rev()
            .collect::<Vec<_>>(),
        ["remote", "get-url", "--push", "--all", "origin"]
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.args.iter().any(|argument| argument == "push"))
    );
}

#[test]
fn scoped_publication_uses_the_bound_url_for_every_network_command() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("owned");
    let worktree = root.join("repo-984321-issue-1240-workflow-2");
    std::fs::create_dir_all(&worktree).unwrap();
    let bound_url = "https://github.com/marmot-protocol/mdk.git";
    let spec = GitPublicationSpec::new_scoped(
        &root,
        &worktree,
        "origin",
        bound_url,
        "pip/v2/repo-984321/issue-1240/workflow-2",
        sha('b'),
        None,
    )
    .unwrap();
    let runner = FakeGit::default();
    runner.push(0, format!("{bound_url}\n"));
    runner.push(0, format!("{}\n", sha('b')));
    runner.push(0, b"pip/v2/repo-984321/issue-1240/workflow-2\n".to_vec());
    runner.push(0, Vec::new());
    runner.push(0, Vec::new());
    runner.push(0, Vec::new());
    runner.push(
        0,
        format!(
            "{}\trefs/heads/pip/v2/repo-984321/issue-1240/workflow-2\n",
            sha('b')
        ),
    );

    assert_eq!(
        publisher(runner.clone()).publish(&spec).unwrap(),
        PublicationResult::Created
    );
    for command in runner.commands.borrow().iter().filter(|command| {
        command
            .args
            .iter()
            .any(|argument| argument == "push" || argument == "ls-remote")
    }) {
        assert!(command.args.iter().any(|argument| argument == bound_url));
        assert!(!command.args.iter().any(|argument| argument == "origin"));
        assert!(
            command
                .args
                .iter()
                .any(|argument| argument == &format!("http.{bound_url}.proxy="))
        );
        assert!(
            command
                .args
                .iter()
                .any(|argument| argument == &format!("http.{bound_url}.sslVerify=true"))
        );
        assert!(
            command
                .args
                .iter()
                .any(|argument| argument == &format!("http.{bound_url}.extraHeader="))
        );
    }
}

#[test]
fn owned_branch_update_uses_an_exact_force_with_lease_and_verifies_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", sha('b')));
    runner.push(0, b"pip/v2/repo-984321/issue-1240/workflow-2\n".to_vec());
    runner.push(0, Vec::new());
    runner.push(
        0,
        format!(
            "{}\trefs/heads/pip/v2/repo-984321/issue-1240/workflow-2\n",
            sha('a')
        ),
    );
    runner.push(0, Vec::new());
    runner.push(
        0,
        format!(
            "{}\trefs/heads/pip/v2/repo-984321/issue-1240/workflow-2\n",
            sha('b')
        ),
    );

    assert_eq!(
        publisher(runner.clone())
            .publish(&spec(tmp.path(), Some(sha('a'))))
            .unwrap(),
        PublicationResult::Updated
    );
    let commands = runner.commands.borrow();
    let push = commands
        .iter()
        .find(|command| command.args.iter().any(|argument| argument == "push"))
        .unwrap();
    assert_eq!(
        push.args,
        [
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "credential.helper=",
            "-c",
            "http.proxy=",
            "-c",
            "http.sslVerify=true",
            "-c",
            "http.followRedirects=initial",
            "-c",
            "http.extraHeader=",
            "-c",
            "remote.origin.proxy=",
            "push",
            "--porcelain",
            "--force-with-lease=refs/heads/pip/v2/repo-984321/issue-1240/workflow-2:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "origin",
            "HEAD:refs/heads/pip/v2/repo-984321/issue-1240/workflow-2",
        ]
    );
}

#[test]
fn successful_retry_is_a_noop_and_remote_race_never_pushes() {
    let tmp = tempfile::tempdir().unwrap();
    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", sha('b')));
    runner.push(0, b"pip/v2/repo-984321/issue-1240/workflow-2\n".to_vec());
    runner.push(0, Vec::new());
    runner.push(
        0,
        format!(
            "{}\trefs/heads/pip/v2/repo-984321/issue-1240/workflow-2\n",
            sha('b')
        ),
    );
    assert_eq!(
        publisher(runner.clone())
            .publish(&spec(tmp.path(), Some(sha('a'))))
            .unwrap(),
        PublicationResult::Existing
    );
    assert!(
        !runner
            .commands
            .borrow()
            .iter()
            .any(|command| command.args.iter().any(|argument| argument == "push"))
    );

    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", sha('b')));
    runner.push(0, b"pip/v2/repo-984321/issue-1240/workflow-2\n".to_vec());
    runner.push(0, Vec::new());
    runner.push(
        0,
        format!(
            "{}\trefs/heads/pip/v2/repo-984321/issue-1240/workflow-2\n",
            sha('c')
        ),
    );
    assert!(matches!(
        publisher(runner.clone()).publish(&spec(tmp.path(), Some(sha('a')))),
        Err(PublicationError::RemoteRace)
    ));
    assert!(
        !runner
            .commands
            .borrow()
            .iter()
            .any(|command| command.args.iter().any(|argument| argument == "push"))
    );
}

#[test]
fn process_publisher_creates_and_updates_a_real_bare_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let repository = tmp.path().join("repository");
    run(
        tmp.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    std::fs::create_dir(&repository).unwrap();
    run(&repository, &["init", "-q", "--initial-branch=main"]);
    run(&repository, &["config", "user.name", "Pip Fixture"]);
    run(
        &repository,
        &["config", "user.email", "pip-fixture@example.invalid"],
    );
    run(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    std::fs::write(repository.join("fixture.txt"), "first\n").unwrap();
    run(&repository, &["add", "fixture.txt"]);
    run(&repository, &["commit", "-qm", "first"]);
    run(
        &repository,
        &["switch", "-qc", "pip/v2/repo-984321/issue-1240/workflow-2"],
    );
    let first = read_sha(&repository);
    let publisher =
        GitPublisher::new(ProcessGitRunner, "git", Duration::from_secs(10), 64 * 1024).unwrap();
    let first_spec = GitPublicationSpec::new_scoped(
        tmp.path(),
        &repository,
        "origin",
        remote.to_str().unwrap(),
        "pip/v2/repo-984321/issue-1240/workflow-2",
        first,
        None,
    )
    .unwrap();
    assert_eq!(
        publisher.publish(&first_spec).unwrap(),
        PublicationResult::Created
    );

    std::fs::write(repository.join("fixture.txt"), "second\n").unwrap();
    run(&repository, &["commit", "-qam", "second"]);
    let second = read_sha(&repository);
    let second_spec = GitPublicationSpec::new_scoped(
        tmp.path(),
        &repository,
        "origin",
        remote.to_str().unwrap(),
        "pip/v2/repo-984321/issue-1240/workflow-2",
        second,
        Some(first),
    )
    .unwrap();
    assert_eq!(
        publisher.publish(&second_spec).unwrap(),
        PublicationResult::Updated
    );
    assert_eq!(remote_sha(&repository), second);
    assert_eq!(
        publisher.publish(&second_spec).unwrap(),
        PublicationResult::Existing
    );
}

fn run(cwd: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .unwrap()
            .success()
    );
}

fn read_sha(repository: &Path) -> GitSha {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository)
        .output()
        .unwrap();
    GitSha::from_str(std::str::from_utf8(&output.stdout).unwrap().trim()).unwrap()
}

fn remote_sha(repository: &Path) -> GitSha {
    let output = Command::new("git")
        .args([
            "ls-remote",
            "--heads",
            "origin",
            "refs/heads/pip/v2/repo-984321/issue-1240/workflow-2",
        ])
        .current_dir(repository)
        .output()
        .unwrap();
    let value = std::str::from_utf8(&output.stdout)
        .unwrap()
        .split_once('\t')
        .unwrap()
        .0;
    GitSha::from_str(value).unwrap()
}
