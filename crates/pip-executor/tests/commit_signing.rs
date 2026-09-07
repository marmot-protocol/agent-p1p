use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use pip_core::GitSha;
use pip_executor::{
    CommitSigningIdentity, GitPublicationSpec, ProcessGitRunner, sign_commit,
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
