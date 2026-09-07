use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use pip_control::{BranchPublicationRequest, BranchPublisher};
use pip_executor::{AllocationError, GitCommand, GitOutput, GitPublisher, GitRunner};

struct NoPublication(Rc<Cell<usize>>);
impl GitRunner for NoPublication {
    fn run(&self, _: &GitCommand) -> Result<GitOutput, AllocationError> {
        self.0.set(self.0.get() + 1);
        Err(AllocationError::Process(
            "authenticated-runner-reached".into(),
        ))
    }
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .envs(pip_executor::workspace_git_environment(
            path,
            pip_executor::sanitized_environment(),
        ))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn worker_configuration_is_validated_before_authenticated_publication() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("case");
    fs::create_dir(&worktree).unwrap();
    let branch = "pip/repo-123/issue-456/workflow-1";
    git(
        &worktree,
        &[
            "init",
            "--quiet",
            "--template=",
            "--shared=group",
            "--initial-branch",
            branch,
        ],
    );
    git(
        &worktree,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    git(
        &worktree,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/fixture.git",
        ],
    );
    fs::write(worktree.join(".git/pip-case-branch"), branch).unwrap();
    let request = BranchPublicationRequest {
        worktree_root: tmp.path().into(),
        worktree: worktree.clone(),
        remote: "origin".into(),
        expected_remote_url: "https://github.com/example/fixture.git".into(),
        branch: branch.into(),
        local_head: git(&worktree, &["rev-parse", "HEAD"]),
        parent_head: git(&worktree, &["rev-parse", "HEAD"]),
        expected_remote_head: None,
    };
    let calls = Rc::new(Cell::new(0));
    let publisher = GitPublisher::new(
        NoPublication(calls.clone()),
        "git",
        Duration::from_secs(10),
        65536,
    )
    .unwrap();
    let error = publisher.publish_branch(&request).unwrap_err();
    assert!(error.to_string().contains("authenticated-runner-reached"));
    assert_eq!(calls.get(), 1);
    calls.set(0);
    git(&worktree, &["config", "filter.hostile.clean", "false"]);
    assert!(publisher.publish_branch(&request).is_err());
    assert_eq!(
        calls.get(),
        0,
        "worker configuration reached credential-bearing runner"
    );
}
