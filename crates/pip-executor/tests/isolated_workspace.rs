use std::fs;
use std::num::{NonZeroU32, NonZeroU64};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use pip_core::{CaseId, IssueNumber, RepositoryId, WorkflowVersion};
use pip_executor::{
    AllocationResult, IsolatedWorkspace, ProcessGitRunner, RetirementResult, WorktreeSpec,
};

use pip_executor::workspace_git_environment;

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .envs(workspace_git_environment(path, Default::default()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

// Invoked three times by the disposable systemd lifecycle test: preparation
// as pip-control, editing under the worker sandbox, then reconciliation and
// retirement as pip-control. No provider, credential, or network is used.
#[test]
#[ignore = "requires distinct service UIDs and systemd; run tests/lifecycle/workspace-handoff.sh"]
fn service_identity_workspace_handoff() {
    let root = std::path::PathBuf::from(std::env::var("PIP_HANDOFF_ROOT").unwrap());
    let phase = std::env::var("PIP_HANDOFF_PHASE").unwrap();
    let case_path = root.join("workspaces/repo-123-issue-456-workflow-1");
    match phase.as_str() {
        "prepare" => {
            let spec = fixture(&root);
            fs::set_permissions(spec.repository(), fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(spec.root(), fs::Permissions::from_mode(0o770)).unwrap();
            allocator().allocate(&spec, REMOTE).unwrap();
            fs::write(root.join("base"), spec.base().to_string()).unwrap();
            fs::write(
                root.join("controller-uid"),
                Command::new("id").arg("-u").output().unwrap().stdout,
            )
            .unwrap();
        }
        "worker" => {
            assert!(fs::read_dir(root.join("private-repository")).is_err());
            assert!(fs::read("/var/lib/pip/ledger.db").is_err());
            assert!(fs::read_dir("/var/lib/pip/hermes").is_err());
            assert!(fs::read_dir("/var/lib/pip/repositories").is_err());
            fs::write(case_path.join("tracked"), "changed by worker\n").unwrap();
            fs::create_dir(case_path.join("new-directory")).unwrap();
            fs::write(case_path.join("new-directory/new-file"), "worker-owned\n").unwrap();
            git(&case_path, &["add", "."]);
            git(
                &case_path,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-qm",
                    "worker commit",
                ],
            );
            git(&case_path, &["fsck", "--no-dangling"]);
            assert!(git(&case_path, &["status", "--porcelain"]).is_empty());
            fs::write(
                case_path.join(".git/worker-uid"),
                Command::new("id").arg("-u").output().unwrap().stdout,
            )
            .unwrap();
        }
        "reconcile" => {
            let case = CaseId::new(
                RepositoryId::new(NonZeroU64::new(123).unwrap()),
                IssueNumber::new(NonZeroU64::new(456).unwrap()),
                WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
            );
            let base = fs::read_to_string(root.join("base")).unwrap();
            let spec = WorktreeSpec::new(
                root.join("private-repository"),
                root.join("workspaces"),
                "pip/",
                case,
                base.parse().unwrap(),
            )
            .unwrap();
            assert_ne!(
                fs::read(root.join("controller-uid")).unwrap(),
                fs::read(case_path.join(".git/worker-uid")).unwrap()
            );
            assert_eq!(
                allocator().allocate(&spec, REMOTE).unwrap(),
                AllocationResult::Existing
            );
            assert_ne!(git(spec.path(), &["rev-parse", "HEAD"]), base);
            assert_eq!(git(spec.repository(), &["rev-parse", "HEAD"]), base);
            assert_eq!(
                fs::read_to_string(case_path.join("new-directory/new-file")).unwrap(),
                "worker-owned\n"
            );
            assert_eq!(
                allocator().retire(&spec.retirement_spec()).unwrap(),
                RetirementResult::Retired
            );
            assert!(spec.repository().is_dir());
        }
        _ => panic!("invalid test phase"),
    }
}

fn fixture(root: &Path) -> WorktreeSpec {
    let source = root.join("private-repository");
    let workspaces = root.join("workspaces");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&workspaces).unwrap();
    git(&source, &["init", "-q", "--initial-branch=main"]);
    fs::write(source.join("tracked"), "original\n").unwrap();
    git(&source, &["add", "tracked"]);
    git(
        &source,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "base",
        ],
    );
    let head = git(&source, &["rev-parse", "HEAD"]).parse().unwrap();
    let case = CaseId::new(
        RepositoryId::new(NonZeroU64::new(123).unwrap()),
        IssueNumber::new(NonZeroU64::new(456).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
    );
    WorktreeSpec::new(source, workspaces, "pip/", case, head).unwrap()
}

fn allocator() -> IsolatedWorkspace<ProcessGitRunner> {
    IsolatedWorkspace::new(ProcessGitRunner, "git", Duration::from_secs(20), 65536).unwrap()
}

const REMOTE: &str = "https://github.com/example/fixture.git";

#[test]
fn independent_case_repository_survives_hidden_canonical_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = fixture(tmp.path());
    assert_eq!(
        allocator().allocate(&spec, REMOTE).unwrap(),
        AllocationResult::Created
    );
    assert!(spec.path().join(".git").is_dir());
    assert!(!spec.path().join(".git/objects/info/alternates").exists());
    assert_eq!(git(spec.path(), &["remote", "get-url", "origin"]), REMOTE);
    assert_eq!(
        git(spec.path(), &["rev-parse", "HEAD"]),
        spec.base().to_string()
    );
    assert_eq!(
        fs::metadata(spec.path()).unwrap().permissions().mode() & 0o7777,
        0o770
    );
    assert_eq!(
        fs::metadata(spec.path().join("tracked"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o660
    );
    for entry in fs::read_dir(spec.path().join(".git/objects/pack")).unwrap() {
        assert_eq!(entry.unwrap().metadata().unwrap().nlink(), 1);
    }
    assert_eq!(
        allocator().allocate(&spec, REMOTE).unwrap(),
        AllocationResult::Existing
    );
    let hidden = tmp.path().join("hidden");
    fs::rename(spec.repository(), &hidden).unwrap();
    fs::write(spec.path().join("tracked"), "builder change\n").unwrap();
    git(spec.path(), &["add", "tracked"]);
    git(
        spec.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "builder",
        ],
    );
    assert!(git(spec.path(), &["status", "--porcelain"]).is_empty());
    assert_eq!(
        git(&hidden, &["rev-parse", "HEAD"]),
        spec.base().to_string()
    );
    let builder_head = git(spec.path(), &["rev-parse", "HEAD"]);
    fs::rename(&hidden, spec.repository()).unwrap();
    assert_eq!(
        allocator().retire(&spec.retirement_spec()).unwrap(),
        RetirementResult::Retired
    );
    assert!(!spec.path().exists());
    assert_eq!(
        git(spec.repository(), &["rev-parse", spec.branch()]),
        builder_head
    );
}

#[test]
fn collisions_dirty_retirement_and_external_git_metadata_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = fixture(tmp.path());
    fs::create_dir(spec.path()).unwrap();
    fs::write(spec.path().join("keep"), "do not remove").unwrap();
    assert!(allocator().allocate(&spec, REMOTE).is_err());
    assert!(spec.path().join("keep").is_file());
    fs::remove_file(spec.path().join("keep")).unwrap();
    fs::remove_dir(spec.path()).unwrap();
    allocator().allocate(&spec, REMOTE).unwrap();
    fs::write(spec.path().join("tracked"), "dirty").unwrap();
    assert!(allocator().retire(&spec.retirement_spec()).is_err());
    assert!(spec.path().exists());
    fs::write(
        spec.path().join(".git/objects/info/alternates"),
        spec.repository().join(".git/objects").to_str().unwrap(),
    )
    .unwrap();
    assert!(allocator().allocate(&spec, REMOTE).is_err());
    assert!(allocator().retire(&spec.retirement_spec()).is_err());
}

#[test]
fn controller_verification_never_executes_worker_fsmonitor_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = fixture(tmp.path());
    allocator().allocate(&spec, REMOTE).unwrap();
    let marker = tmp.path().join("unexpected-execution");
    let hook = tmp.path().join("fsmonitor");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nprintf '\\0'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    git(
        spec.path(),
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
    );
    pip_executor::CheckoutReconciler::process(
        "git",
        pip_executor::sanitized_environment(),
        Duration::from_secs(10),
        65536,
    )
    .unwrap()
    .verify_worktree(spec.path(), spec.branch(), spec.base())
    .unwrap();
    assert!(!marker.exists());
}

#[test]
fn cross_uid_git_trust_is_exact_and_replaces_inherited_overrides() {
    let path = Path::new("/bound/case");
    let env = workspace_git_environment(
        path,
        [
            ("GIT_CONFIG_COUNT".into(), "1".into()),
            ("GIT_CONFIG_KEY_0".into(), "safe.directory".into()),
            ("GIT_CONFIG_VALUE_0".into(), "*".into()),
            ("GIT_CONFIG_PARAMETERS".into(), "'safe.directory=*'".into()),
            ("HOME".into(), "/provider-home".into()),
        ]
        .into(),
    );
    assert_eq!(env["GIT_CONFIG_COUNT"], "4");
    assert_eq!(env["GIT_CONFIG_VALUE_0"], "/bound/case");
    assert_eq!(env["HOME"], "/provider-home");
    assert!(!env.contains_key("GIT_CONFIG_PARAMETERS"));
    assert!(!env.values().any(|value| value == "*"));
}

#[test]
fn retirement_preserves_workspace_when_the_retained_branch_diverged() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = fixture(tmp.path());
    allocator().allocate(&spec, REMOTE).unwrap();
    fs::write(spec.path().join("tracked"), "builder\n").unwrap();
    git(
        spec.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qam",
            "builder",
        ],
    );
    git(spec.repository(), &["checkout", "-qb", spec.branch()]);
    fs::write(spec.repository().join("tracked"), "independent change\n").unwrap();
    git(
        spec.repository(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qam",
            "independent",
        ],
    );
    let retained = git(spec.repository(), &["rev-parse", "HEAD"]);
    git(spec.repository(), &["checkout", "-q", "main"]);
    assert!(allocator().retire(&spec.retirement_spec()).is_err());
    assert!(spec.path().is_dir());
    assert_eq!(
        git(spec.repository(), &["rev-parse", spec.branch()]),
        retained
    );
}

#[test]
fn tracked_symlinks_are_not_followed_when_sharing_or_retiring() {
    let tmp = tempfile::tempdir().unwrap();
    let original = fixture(tmp.path());
    let sentinel = tmp.path().join("private-sentinel");
    fs::write(&sentinel, "private").unwrap();
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&sentinel, original.repository().join("link")).unwrap();
    git(original.repository(), &["add", "link"]);
    git(
        original.repository(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "symlink",
        ],
    );
    let case = CaseId::new(
        RepositoryId::new(NonZeroU64::new(123).unwrap()),
        IssueNumber::new(NonZeroU64::new(456).unwrap()),
        WorkflowVersion::new(NonZeroU32::new(1).unwrap()),
    );
    let spec = WorktreeSpec::new(
        original.repository(),
        original.root(),
        "pip/",
        case,
        git(original.repository(), &["rev-parse", "HEAD"])
            .parse()
            .unwrap(),
    )
    .unwrap();
    allocator().allocate(&spec, REMOTE).unwrap();
    assert!(
        fs::symlink_metadata(spec.path().join("link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    allocator().retire(&spec.retirement_spec()).unwrap();
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "private");
    assert_eq!(
        fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn legacy_linked_worktree_requires_explicit_recovery_without_permission_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = fixture(tmp.path());
    pip_executor::WorktreeAllocator::new(ProcessGitRunner, "git", Duration::from_secs(10), 65536)
        .unwrap()
        .allocate(&spec)
        .unwrap();
    fs::set_permissions(spec.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let pointer = fs::read(spec.path().join(".git")).unwrap();
    assert!(allocator().allocate(&spec, REMOTE).is_err());
    assert_eq!(fs::read(spec.path().join(".git")).unwrap(), pointer);
    assert_eq!(
        fs::metadata(spec.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        git(spec.path(), &["rev-parse", "HEAD"]),
        spec.base().to_string()
    );
}

#[test]
fn worker_config_cannot_enable_filters_includes_or_url_rewrites_for_controller_git() {
    for (key, value) in [
        ("filter.unsafe.clean", "touch /tmp/should-not-execute"),
        ("include.path", "/etc/private-config"),
        (
            "url.http://untrusted.invalid/.insteadOf",
            "https://github.com/",
        ),
        (
            "uploadpack.packObjectsHook",
            "touch /tmp/should-not-execute",
        ),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let spec = fixture(tmp.path());
        allocator().allocate(&spec, REMOTE).unwrap();
        git(spec.path(), &["config", key, value]);
        assert!(
            allocator().allocate(&spec, REMOTE).is_err(),
            "accepted {key}"
        );
        assert!(
            allocator().retire(&spec.retirement_spec()).is_err(),
            "retired with {key}"
        );
        assert!(spec.path().is_dir());
    }
}
