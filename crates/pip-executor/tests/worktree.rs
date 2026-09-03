use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use pip_core::{CaseId, GitSha, IssueNumber, RepositoryId, WorkflowVersion};
use pip_executor::{
    AllocationError, AllocationResult, GitCommand, GitOutput, GitRunner, ProcessGitRunner,
    RetirementResult, WorktreeAllocator, WorktreeRetirer, WorktreeSpec,
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

fn case_id() -> CaseId {
    CaseId::new(
        RepositoryId::new(NonZeroU64::new(984_321).unwrap()),
        IssueNumber::new(NonZeroU64::new(1_240).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(2).unwrap()),
    )
}

fn base() -> GitSha {
    GitSha::from_str(&"a".repeat(40)).unwrap()
}

fn worktree_entry(path: &Path, branch: &str, head: &str) -> Vec<u8> {
    format!(
        "worktree {}\0HEAD {}\0branch refs/heads/{}\0\0",
        path.display(),
        head,
        branch
    )
    .into_bytes()
}

fn prepare(tmp: &tempfile::TempDir) -> (PathBuf, PathBuf, WorktreeSpec) {
    let repository = tmp.path().join("repo");
    let root = tmp.path().join("managed-worktrees");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let spec = WorktreeSpec::new(&repository, &root, "pip/", case_id(), base()).unwrap();
    (repository, root, spec)
}

#[test]
fn names_are_deterministic_and_remain_under_the_controller_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, root, spec) = prepare(&tmp);
    assert_eq!(spec.branch(), "pip/repo-984321/issue-1240/workflow-2");
    assert_eq!(
        spec.path(),
        root.canonicalize()
            .unwrap()
            .join("repo-984321-issue-1240-workflow-2")
    );
    assert!(spec.path().starts_with(root.canonicalize().unwrap()));

    assert!(matches!(
        WorktreeSpec::new(spec.repository(), &root, "../foreign/", case_id(), base()),
        Err(AllocationError::InvalidSpec)
    ));
}

#[test]
fn absent_branch_and_worktree_are_created_once_from_the_exact_base() {
    let tmp = tempfile::tempdir().unwrap();
    let (repository, _, spec) = prepare(&tmp);
    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", repository.display()));
    runner.push(0, Vec::new());
    runner.push(0, worktree_entry(&repository, "main", &base().to_string()));
    runner.push(1, Vec::new());
    runner.push(0, Vec::new());
    runner.push(
        0,
        worktree_entry(spec.path(), spec.branch(), &base().to_string()),
    );
    let allocator =
        WorktreeAllocator::new(runner.clone(), "git", Duration::from_secs(5), 16_384).unwrap();

    assert_eq!(
        allocator.allocate(&spec).unwrap(),
        AllocationResult::Created
    );
    let commands = runner.commands.borrow();
    let create = commands
        .iter()
        .find(|command| {
            command.args.first().map(String::as_str) == Some("worktree")
                && command.args.get(1).map(String::as_str) == Some("add")
        })
        .unwrap();
    assert_eq!(
        create.args,
        [
            "worktree",
            "add",
            "--no-track",
            "-b",
            spec.branch(),
            spec.path().to_str().unwrap(),
            &base().to_string(),
        ]
    );
}

#[test]
fn exact_existing_worktree_is_a_noop_even_after_the_branch_advances() {
    let tmp = tempfile::tempdir().unwrap();
    let (repository, _, spec) = prepare(&tmp);
    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", repository.display()));
    runner.push(0, Vec::new());
    runner.push(
        0,
        worktree_entry(spec.path(), spec.branch(), &"b".repeat(40)),
    );
    let allocator =
        WorktreeAllocator::new(runner.clone(), "git", Duration::from_secs(5), 16_384).unwrap();

    assert_eq!(
        allocator.allocate(&spec).unwrap(),
        AllocationResult::Existing
    );
    assert!(
        !runner
            .commands
            .borrow()
            .iter()
            .any(|command| command.args.get(1).map(String::as_str) == Some("add"))
    );
}

#[test]
fn path_or_branch_collision_fails_without_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let (repository, root, spec) = prepare(&tmp);
    let other_path = root.canonicalize().unwrap().join("other");
    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", repository.display()));
    runner.push(0, Vec::new());
    let mut listing = worktree_entry(spec.path(), "foreign", &base().to_string());
    listing.extend(worktree_entry(
        &other_path,
        spec.branch(),
        &base().to_string(),
    ));
    runner.push(0, listing);
    let allocator =
        WorktreeAllocator::new(runner.clone(), "git", Duration::from_secs(5), 16_384).unwrap();

    assert!(matches!(
        allocator.allocate(&spec),
        Err(AllocationError::Collision)
    ));
    assert_eq!(runner.commands.borrow().len(), 3);
}

#[test]
fn unattached_existing_case_branch_is_reattached_without_recreating_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (repository, _, spec) = prepare(&tmp);
    let runner = FakeGit::default();
    runner.push(0, format!("{}\n", repository.display()));
    runner.push(0, Vec::new());
    runner.push(0, worktree_entry(&repository, "main", &base().to_string()));
    runner.push(0, Vec::new());
    runner.push(0, Vec::new());
    runner.push(
        0,
        worktree_entry(spec.path(), spec.branch(), &"b".repeat(40)),
    );
    let allocator =
        WorktreeAllocator::new(runner.clone(), "git", Duration::from_secs(5), 16_384).unwrap();

    assert_eq!(
        allocator.allocate(&spec).unwrap(),
        AllocationResult::Created
    );
    let commands = runner.commands.borrow();
    let attach = commands
        .iter()
        .find(|command| command.args.get(1).map(String::as_str) == Some("add"))
        .unwrap();
    assert_eq!(
        attach.args,
        [
            "worktree",
            "add",
            "--no-track",
            spec.path().to_str().unwrap(),
            spec.branch(),
        ]
    );
}

#[test]
fn process_allocator_creates_and_reconciles_a_real_git_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = tmp.path().join("repo");
    let root = tmp.path().join("managed-worktrees");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    for args in [
        vec!["init", "-q", "--initial-branch=main"],
        vec!["config", "user.name", "Pip Fixture"],
        vec!["config", "user.email", "pip-fixture@example.invalid"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repository.join("fixture.txt"), "fixture\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "fixture.txt"])
            .current_dir(&repository)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-qm", "fixture"])
            .current_dir(&repository)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repository)
        .output()
        .unwrap();
    let sha = GitSha::from_str(std::str::from_utf8(&output.stdout).unwrap().trim()).unwrap();
    let spec = WorktreeSpec::new(&repository, &root, "pip/", case_id(), sha).unwrap();
    let allocator =
        WorktreeAllocator::new(ProcessGitRunner, "git", Duration::from_secs(10), 65_536).unwrap();

    assert_eq!(
        allocator.allocate(&spec).unwrap(),
        AllocationResult::Created
    );
    assert_eq!(
        allocator.allocate(&spec).unwrap(),
        AllocationResult::Existing
    );
    assert!(spec.path().join(".git").is_file());
    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(spec.path())
        .output()
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&branch.stdout).unwrap().trim(),
        spec.branch()
    );
}

#[test]
fn retirement_is_non_forcing_idempotent_and_refuses_dirty_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let repository = tmp.path().join("repo");
    let root = tmp.path().join("managed-worktrees");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    for args in [
        vec!["init", "-q", "--initial-branch=main"],
        vec!["config", "user.name", "Pip Fixture"],
        vec!["config", "user.email", "pip-fixture@example.invalid"],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&repository)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(repository.join("fixture.txt"), "fixture\n").unwrap();
    assert!(
        Command::new("git")
            .args(["add", "fixture.txt"])
            .current_dir(&repository)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["commit", "-qm", "fixture"])
            .current_dir(&repository)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repository)
        .output()
        .unwrap();
    let sha = GitSha::from_str(std::str::from_utf8(&output.stdout).unwrap().trim()).unwrap();
    let spec = WorktreeSpec::new(&repository, &root, "pip/", case_id(), sha).unwrap();
    WorktreeAllocator::new(ProcessGitRunner, "git", Duration::from_secs(10), 65_536)
        .unwrap()
        .allocate(&spec)
        .unwrap();
    let retirer =
        WorktreeRetirer::new(ProcessGitRunner, "git", Duration::from_secs(10), 65_536).unwrap();

    std::fs::write(spec.path().join("untracked.txt"), "preserve me\n").unwrap();
    assert!(matches!(
        retirer.retire(&spec.retirement_spec()),
        Err(AllocationError::DirtyWorktree)
    ));
    assert!(spec.path().join("untracked.txt").is_file());

    std::fs::remove_file(spec.path().join("untracked.txt")).unwrap();
    assert_eq!(
        retirer.retire(&spec.retirement_spec()).unwrap(),
        RetirementResult::Retired
    );
    assert!(!spec.path().exists());
    assert_eq!(
        retirer.retire(&spec.retirement_spec()).unwrap(),
        RetirementResult::Absent
    );
}
