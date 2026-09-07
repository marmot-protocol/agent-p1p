use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use pip_core::GitSha;
use pip_executor::{
    AllocationError, CommitSigningIdentity, GitCommand, GitOutput, GitPublicationSpec,
    GitPublisher, GitRunner, ProcessGitRunner, PublicationError, PublicationResult, sign_commit,
    workspace_git_environment,
};

const BRANCH: &str = "pip/repo-123/issue-45/workflow-1";
const REMOTE: &str = "https://github.com/example/repository.git";

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .envs(workspace_git_environment(path, Default::default()))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[derive(Clone)]
struct LocalRemote {
    path: PathBuf,
    fail_push: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl GitRunner for LocalRemote {
    fn run(&self, command: &GitCommand) -> Result<GitOutput, AllocationError> {
        let mut command = command.clone();
        if command.args.iter().any(|arg| arg == "push") {
            assert!(
                command.args.iter().any(|arg| {
                    arg.split_once(":refs/heads/")
                        .is_some_and(|(sha, _)| sha.parse::<GitSha>().is_ok())
                }),
                "push must name the immutable signed commit, not HEAD"
            );
            if self.fail_push.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(GitOutput {
                    status: 1,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    timed_out: false,
                });
            }
        }
        if command
            .args
            .iter()
            .any(|arg| arg == "push" || arg == "ls-remote")
        {
            for arg in &mut command.args {
                if arg == REMOTE {
                    *arg = self.path.to_str().unwrap().to_owned();
                }
            }
        }
        ProcessGitRunner.run(&command)
    }
}

#[test]
fn signed_publication_retains_source_and_recovers_an_interrupted_push() {
    let f = Fixture::new();
    let remote = f._temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-q"]);
    let fail_push = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let publisher = GitPublisher::new(
        LocalRemote {
            path: remote.clone(),
            fail_push: fail_push.clone(),
        },
        "git",
        std::time::Duration::from_secs(30),
        1024 * 1024,
    )
    .unwrap();
    assert!(matches!(
        publisher.publish_signed(&f.spec, f.base, &f.identity, &f.key),
        Err(PublicationError::CommandFailed(1))
    ));
    let signed_head = git(&f.worktree, &["rev-parse", "HEAD"]);
    assert_ne!(signed_head, f.spec.local_head().to_string());
    let retained = format!("refs/pip/source-builds/{}", f.spec.local_head());
    assert_eq!(
        git(&f.worktree, &["rev-parse", &retained]),
        f.spec.local_head().to_string()
    );
    assert!(git(&f.worktree, &["status", "--porcelain"]).is_empty());
    // Neither GC nor losing the branch's source-head reflog may lose accepted work.
    git(&f.worktree, &["reflog", "expire", "--expire=now", "--all"]);
    git(&f.worktree, &["gc", "--prune=now", "--quiet"]);
    assert_eq!(
        git(
            &f.worktree,
            &["cat-file", "-t", &f.spec.local_head().to_string()]
        ),
        "commit"
    );
    fail_push.store(false, std::sync::atomic::Ordering::SeqCst);
    let (result, signed) = publisher
        .publish_signed(&f.spec, f.base, &f.identity, &f.key)
        .unwrap();
    assert_eq!(result, PublicationResult::Created);
    assert_eq!(signed.head.to_string(), signed_head);
    assert_eq!(
        git(&remote, &["rev-parse", &format!("refs/heads/{BRANCH}")]),
        signed_head
    );
    let (result, replay) = publisher
        .publish_signed(&f.spec, f.base, &f.identity, &f.key)
        .unwrap();
    assert_eq!(result, PublicationResult::Existing);
    assert_eq!(signed, replay);
    git(
        &remote,
        &[
            "update-ref",
            &format!("refs/heads/{BRANCH}"),
            &f.base.to_string(),
            &signed_head,
        ],
    );
    assert!(matches!(
        publisher.publish_signed(&f.spec, f.base, &f.identity, &f.key),
        Err(PublicationError::RemoteRace)
    ));
    assert_eq!(
        git(&remote, &["rev-parse", &format!("refs/heads/{BRANCH}")]),
        f.base.to_string()
    );
}

#[test]
fn signed_publication_rejects_a_conflicting_retained_source_ref() {
    let f = Fixture::new();
    let retained = format!("refs/pip/source-builds/{}", f.spec.local_head());
    git(&f.worktree, &["update-ref", &retained, &f.base.to_string()]);
    let publisher = GitPublisher::new(
        ProcessGitRunner,
        "git",
        std::time::Duration::from_secs(30),
        1024 * 1024,
    )
    .unwrap();
    assert!(matches!(
        publisher.publish_signed(&f.spec, f.base, &f.identity, &f.key),
        Err(PublicationError::VerificationFailed)
    ));
    assert_eq!(
        git(&f.worktree, &["rev-parse", "HEAD"]),
        f.spec.local_head().to_string()
    );
    assert_eq!(
        git(&f.worktree, &["rev-parse", &retained]),
        f.base.to_string()
    );
}

#[test]
fn signed_git_metadata_remains_group_accessible_under_the_controller_private_umask() {
    const CHILD: &str = "PIP_TEST_SIGNING_PRIVATE_UMASK";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new("/bin/sh")
            .args(["-c", "umask 0077; exec \"$@\"", "pip-signing-umask-test"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "signed_git_metadata_remains_group_accessible_under_the_controller_private_umask",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let f = Fixture::new();
    let remote = f._temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-q"]);
    let publisher = GitPublisher::new(
        LocalRemote {
            path: remote,
            fail_push: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        "git",
        std::time::Duration::from_secs(30),
        1024 * 1024,
    )
    .unwrap();
    let (_, signed) = publisher
        .publish_signed(&f.spec, f.base, &f.identity, &f.key)
        .unwrap();
    let head = signed.head.to_string();
    for relative in [
        format!(".git/objects/{}/{}", &head[..2], &head[2..]),
        format!(".git/refs/pip/source-builds/{}", signed.source_head),
        format!(".git/refs/heads/{BRANCH}"),
        format!(".git/logs/refs/heads/{BRANCH}"),
        ".git/logs/HEAD".into(),
    ] {
        let path = f.worktree.join(relative);
        let required = if path.to_string_lossy().contains("/.git/objects/") {
            0o040
        } else {
            0o060
        };
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & required,
            required,
            "worker cannot read {}",
            path.display()
        );
        for parent in path
            .parent()
            .unwrap()
            .ancestors()
            .take_while(|parent| *parent != f.worktree)
        {
            assert_eq!(
                fs::metadata(parent).unwrap().permissions().mode() & 0o070,
                0o070,
                "worker cannot use {}",
                parent.display()
            );
        }
    }
    assert_eq!(
        fs::metadata(&f.key).unwrap().permissions().mode() & 0o077,
        0
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    worktree: PathBuf,
    key: PathBuf,
    identity: CommitSigningIdentity,
    spec: GitPublicationSpec,
    base: GitSha,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("worktrees");
        let worktree = root.join("case");
        fs::create_dir_all(&worktree).unwrap();
        git(&worktree, &["init", "-q", "--initial-branch", BRANCH]);
        git(&worktree, &["remote", "add", "origin", REMOTE]);
        fs::write(worktree.join(".git/pip-case-branch"), BRANCH).unwrap();
        git(
            &worktree,
            &[
                "-c",
                "user.name=Worker",
                "-c",
                "user.email=worker@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "base",
            ],
        );
        let base = git(&worktree, &["rev-parse", "HEAD"]).parse().unwrap();
        fs::write(worktree.join("binary"), [0, 255, 13, 10]).unwrap();
        fs::write(worktree.join("executable"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            worktree.join("executable"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("binary", worktree.join("link")).unwrap();
        git(&worktree, &["add", "."]);
        git(
            &worktree,
            &[
                "-c",
                "user.name=Worker",
                "-c",
                "user.email=worker@example.invalid",
                "commit",
                "-qm",
                "accepted builder work",
            ],
        );
        let source = git(&worktree, &["rev-parse", "HEAD"]).parse().unwrap();
        let key = temp.path().join("signing-key");
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        let identity = CommitSigningIdentity {
            name: "Pip Fixture".into(),
            email: "fixture@example.invalid".into(),
            public_key: fs::read_to_string(key.with_extension("pub")).unwrap(),
        };
        let spec = GitPublicationSpec::new_scoped(
            &root, &worktree, "origin", REMOTE, BRANCH, source, None,
        )
        .unwrap();
        Self {
            _temp: temp,
            worktree,
            key,
            identity,
            spec,
            base,
        }
    }
}

#[test]
fn signing_preserves_the_exact_tree_and_source_history_and_is_deterministic() {
    let f = Fixture::new();
    let before = git(&f.worktree, &["show-ref"]);
    let config = fs::read(f.worktree.join(".git/config")).unwrap();
    let signed = sign_commit(
        ProcessGitRunner,
        "git",
        &f.spec,
        f.base,
        &f.identity,
        &f.key,
    )
    .unwrap();
    assert_ne!(signed.head, f.spec.local_head());
    assert_eq!(signed.source_head, f.spec.local_head());
    assert_eq!(
        signed.tree.to_string(),
        git(&f.worktree, &["rev-parse", "HEAD^{tree}"])
    );
    assert_eq!(signed.parent, f.base);
    assert_eq!(
        before,
        git(&f.worktree, &["show-ref"]),
        "signing must not move any ref or rewrite accepted work"
    );
    assert_eq!(
        config,
        fs::read(f.worktree.join(".git/config")).unwrap(),
        "never persist a credential-bearing Git configuration"
    );
    assert_eq!(
        signed,
        sign_commit(
            ProcessGitRunner,
            "git",
            &f.spec,
            f.base,
            &f.identity,
            &f.key
        )
        .unwrap()
    );
    let object = git(
        &f.worktree,
        &["cat-file", "commit", &signed.head.to_string()],
    );
    assert!(object.contains("gpgsig -----BEGIN SSH SIGNATURE-----"));
    assert!(object.contains(&format!("Pip-Source-Commit: {}", f.spec.local_head())));
    assert!(git(&f.worktree, &["status", "--porcelain"]).is_empty());
}

#[test]
fn a_worker_cannot_substitute_commit_bytes_under_the_accepted_object_name() {
    let f = Fixture::new();
    let original = f.spec.local_head().to_string();
    let raw = git(&f.worktree, &["cat-file", "commit", &original]);
    let forged = f._temp.path().join("forged-commit");
    fs::write(
        &forged,
        raw.replace("committer Worker", "committer Imposter") + "\n",
    )
    .unwrap();
    let replacement = git(
        &f.worktree,
        &[
            "hash-object",
            "-w",
            "-t",
            "commit",
            forged.to_str().unwrap(),
        ],
    );
    assert_ne!(replacement, original);
    let object = |sha: &str| {
        f.worktree
            .join(".git/objects")
            .join(&sha[..2])
            .join(&sha[2..])
    };
    fs::set_permissions(object(&original), fs::Permissions::from_mode(0o600)).unwrap();
    fs::copy(object(&replacement), object(&original)).unwrap();
    assert!(
        sign_commit(
            ProcessGitRunner,
            "git",
            &f.spec,
            f.base,
            &f.identity,
            &f.key
        )
        .is_err(),
        "the source object name alone is not proof of the accepted commit's bytes"
    );
}

#[test]
fn signing_rejects_corrupted_blob_content_even_when_the_checkout_is_clean() {
    let f = Fixture::new();
    let original = git(&f.worktree, &["rev-parse", "HEAD:binary"]);
    let forged = f._temp.path().join("forged-blob");
    fs::write(&forged, b"different bytes").unwrap();
    let replacement = git(
        &f.worktree,
        &["hash-object", "-w", forged.to_str().unwrap()],
    );
    let object = |sha: &str| {
        f.worktree
            .join(".git/objects")
            .join(&sha[..2])
            .join(&sha[2..])
    };
    fs::set_permissions(object(&original), fs::Permissions::from_mode(0o600)).unwrap();
    fs::copy(object(&replacement), object(&original)).unwrap();
    assert!(git(&f.worktree, &["status", "--porcelain"]).is_empty());
    assert!(
        sign_commit(
            ProcessGitRunner,
            "git",
            &f.spec,
            f.base,
            &f.identity,
            &f.key
        )
        .is_err(),
        "signing must verify the accepted tree's reachable object contents"
    );
}

#[test]
fn signing_rejects_mismatched_keys_unsafe_config_dirty_work_and_identity_injection() {
    for fault in [
        "key", "mode", "symlink", "config", "dirty", "identity", "head", "remote",
    ] {
        let mut f = Fixture::new();
        match fault {
            "key" => {
                let other = Fixture::new();
                f.identity.public_key = other.identity.public_key.clone();
            }
            "mode" => fs::set_permissions(&f.key, fs::Permissions::from_mode(0o644)).unwrap(),
            "symlink" => {
                let link = f._temp.path().join("key-link");
                symlink(&f.key, &link).unwrap();
                f.key = link;
            }
            "config" => {
                git(
                    &f.worktree,
                    &["config", "gpg.ssh.program", "/untrusted-signer"],
                );
            }
            "dirty" => fs::write(f.worktree.join("binary"), "not committed").unwrap(),
            "identity" => f.identity.email.push_str("\nattacker@example.invalid"),
            "head" => {
                git(
                    &f.worktree,
                    &[
                        "-c",
                        "user.name=Worker",
                        "-c",
                        "user.email=worker@example.invalid",
                        "commit",
                        "--allow-empty",
                        "-qm",
                        "unaccepted",
                    ],
                );
            }
            "remote" => {
                git(
                    &f.worktree,
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        "https://example.invalid/foreign.git",
                    ],
                );
            }
            _ => unreachable!(),
        }
        let refs = git(&f.worktree, &["show-ref"]);
        assert!(
            sign_commit(
                ProcessGitRunner,
                "git",
                &f.spec,
                f.base,
                &f.identity,
                &f.key
            )
            .is_err(),
            "{fault} must block signing"
        );
        assert_eq!(refs, git(&f.worktree, &["show-ref"]));
    }
}

#[test]
fn replacement_refs_cannot_change_the_signed_build_tree() {
    let f = Fixture::new();
    let source_tree = git(&f.worktree, &["rev-parse", "HEAD^{tree}"]);
    git(
        &f.worktree,
        &[
            "replace",
            &f.spec.local_head().to_string(),
            &f.base.to_string(),
        ],
    );
    assert_ne!(source_tree, git(&f.worktree, &["rev-parse", "HEAD^{tree}"]));
    let signed = sign_commit(
        ProcessGitRunner,
        "git",
        &f.spec,
        f.base,
        &f.identity,
        &f.key,
    )
    .unwrap();
    assert_eq!(signed.tree.to_string(), source_tree);
}

#[test]
fn signing_rechecks_path_ownership_after_the_publication_spec_was_constructed() {
    let f = Fixture::new();
    let root = f.worktree.parent().unwrap();
    let moved = f._temp.path().join("foreign-root");
    fs::rename(root, &moved).unwrap();
    symlink(&moved, root).unwrap();
    assert!(
        sign_commit(
            ProcessGitRunner,
            "git",
            &f.spec,
            f.base,
            &f.identity,
            &f.key
        )
        .is_err(),
        "an ancestor symlink must not redirect signing outside the bound workspace"
    );
}

#[test]
fn signing_replays_after_publication_has_aligned_the_local_branch() {
    let f = Fixture::new();
    let signed = sign_commit(
        ProcessGitRunner,
        "git",
        &f.spec,
        f.base,
        &f.identity,
        &f.key,
    )
    .unwrap();
    git(
        &f.worktree,
        &[
            "update-ref",
            &format!("refs/heads/{BRANCH}"),
            &signed.head.to_string(),
            &f.spec.local_head().to_string(),
        ],
    );
    assert_eq!(
        sign_commit(
            ProcessGitRunner,
            "git",
            &f.spec,
            f.base,
            &f.identity,
            &f.key
        )
        .unwrap(),
        signed
    );
    assert_eq!(
        git(&f.worktree, &["rev-parse", "HEAD"]),
        signed.head.to_string()
    );
}
