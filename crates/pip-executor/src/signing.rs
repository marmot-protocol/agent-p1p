//! Controller-side signing of accepted build trees, without rewriting worker results.

use crate::{
    GitCommand, GitPublicationSpec, GitRunner, IsolatedWorkspace, PublicationError,
    workspace_git_environment,
};
use pip_core::GitSha;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitSigningIdentity {
    pub name: String,
    pub email: String,
    pub public_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedCommit {
    pub source_head: GitSha,
    pub tree: GitSha,
    pub head: GitSha,
    pub parent: GitSha,
    pub signer_fingerprint: String,
}

/// Requires exclusive controller ownership of the workspace execution slot.
/// Writes a Git object only; the caller retains the source commit and records
/// the returned binding durably before replacing refs or releasing reviewers.
pub fn sign_commit<R: GitRunner + Clone>(
    runner: R,
    git: &str,
    spec: &GitPublicationSpec,
    parent: GitSha,
    identity: &CommitSigningIdentity,
    key: &Path,
) -> Result<SignedCommit, PublicationError> {
    let remote = spec
        .expected_remote_url()
        .ok_or(PublicationError::InvalidSpec)?;
    if spec.worktree().canonicalize().map_err(io_error)? != spec.worktree() {
        return Err(PublicationError::InvalidSpec);
    }
    let mut fields = identity.public_key.split_whitespace();
    let algorithm = fields.next().unwrap_or_default();
    let public = fields.next().unwrap_or_default();
    if git.is_empty()
        || identity.name.trim().is_empty()
        || identity.name.len() > 128
        || identity
            .name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>'))
        || !identity.email.contains('@')
        || identity.email.len() > 254
        || !identity
            .email
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"@._+-".contains(&ch))
        || algorithm != "ssh-ed25519"
        || public.is_empty()
        || public.len() > 256
        || !public
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"+/=".contains(&ch))
        || !key.is_absolute()
    {
        return Err(PublicationError::InvalidConfiguration);
    }
    let metadata = fs::symlink_metadata(key).map_err(io_error)?;
    let mode = metadata.permissions().mode() & 0o7777;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > 16 * 1024
        || !(matches!(mode, 0o400 | 0o600)
            || mode == 0o440 && metadata.uid() == 0 && metadata.gid() == 0)
    {
        return Err(PublicationError::InvalidConfiguration);
    }
    let key = key.canonicalize().map_err(io_error)?;
    if key.starts_with(
        spec.worktree()
            .parent()
            .ok_or(PublicationError::InvalidSpec)?,
    ) {
        return Err(PublicationError::InvalidConfiguration);
    }
    // Reject executable repository configuration before any process receives
    // the signing credential path. Signing never runs a worker or a Git hook.
    IsolatedWorkspace::new(runner.clone(), git, Duration::from_secs(30), 1024 * 1024)
        .and_then(|workspace| {
            workspace.verify_for_controller(spec.worktree(), spec.branch(), remote)
        })
        .map_err(|error| PublicationError::Process(error.to_string()))?;
    let environment = workspace_git_environment(
        spec.worktree(),
        BTreeMap::from([("GIT_NO_REPLACE_OBJECTS".into(), "1".into())]),
    );
    let run =
        |args: Vec<String>, extra: &BTreeMap<String, String>| -> Result<String, PublicationError> {
            let mut environment = environment.clone();
            environment.extend(extra.clone());
            let output = runner
                .run(&GitCommand {
                    program: git.into(),
                    cwd: spec.worktree().into(),
                    args,
                    timeout: Duration::from_secs(30),
                    max_output_bytes: 1024 * 1024,
                    environment,
                })
                .map_err(|error| PublicationError::Process(error.to_string()))?;
            if output.timed_out {
                return Err(PublicationError::TimedOut);
            }
            if output.stdout.len() > 1024 * 1024 || output.stderr.len() > 1024 * 1024 {
                return Err(PublicationError::OutputTooLarge);
            }
            if output.status != 0 {
                return Err(PublicationError::CommandFailed(output.status));
            }
            String::from_utf8(output.stdout)
                .map(|value| value.trim_end_matches('\n').to_owned())
                .map_err(|error| PublicationError::MalformedOutput(error.to_string()))
        };
    let plain = BTreeMap::new();
    let source = spec.local_head().to_string();
    let current_head = run(vec!["rev-parse".into(), "HEAD".into()], &plain)?;
    if !run(
        vec!["status".into(), "--porcelain=v1".into(), "-z".into()],
        &plain,
    )?
    .is_empty()
    {
        return Err(PublicationError::DirtyWorktree);
    }
    // A worker can own loose Git objects. A clean index is not evidence that
    // the accepted tree's reachable object contents still match their hashes.
    run(
        vec![
            "fsck".into(),
            "--full".into(),
            "--no-reflogs".into(),
            "--no-dangling".into(),
            "--no-progress".into(),
            source.clone(),
        ],
        &plain,
    )?;
    run(
        vec![
            "merge-base".into(),
            "--is-ancestor".into(),
            parent.to_string(),
            source.clone(),
        ],
        &plain,
    )?;
    let tree: GitSha = run(
        vec!["rev-parse".into(), format!("{source}^{{tree}}")],
        &plain,
    )?
    .parse()
    .map_err(|_| PublicationError::VerificationFailed)?;
    let timestamp = run(
        vec![
            "show".into(),
            "-s".into(),
            "--format=%ct".into(),
            source.clone(),
        ],
        &plain,
    )?;
    timestamp
        .parse::<u64>()
        .map_err(|_| PublicationError::VerificationFailed)?;
    let signer_environment = BTreeMap::from([
        ("GIT_AUTHOR_NAME".into(), identity.name.clone()),
        ("GIT_AUTHOR_EMAIL".into(), identity.email.clone()),
        ("GIT_COMMITTER_NAME".into(), identity.name.clone()),
        ("GIT_COMMITTER_EMAIL".into(), identity.email.clone()),
        ("GIT_AUTHOR_DATE".into(), format!("{timestamp} +0000")),
        ("GIT_COMMITTER_DATE".into(), format!("{timestamp} +0000")),
    ]);
    let temporary = tempfile::Builder::new()
        .prefix("pip-sign-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .map_err(io_error)?;
    let allowed = temporary.path().join("allowed-signers");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&allowed)
        .map_err(io_error)?;
    writeln!(
        file,
        "{} namespaces=\"git\" {algorithm} {public}",
        identity.email
    )
    .map_err(io_error)?;
    drop(file);
    let signing_args = vec![
        "-c".into(),
        "gpg.format=ssh".into(),
        "-c".into(),
        "gpg.ssh.program=/usr/bin/ssh-keygen".into(),
        "-c".into(),
        format!("user.signingkey={}", key.display()),
        "-c".into(),
        format!("gpg.ssh.allowedSignersFile={}", allowed.display()),
    ];
    let message = format!(
        "Pip: publish accepted build\n\nPip-Source-Commit: {source}\nPip-Source-Tree: {tree}\n"
    );
    let mut args = signing_args.clone();
    args.extend([
        "commit-tree".into(),
        "-S".into(),
        "-p".into(),
        parent.to_string(),
        "-m".into(),
        message,
        tree.to_string(),
    ]);
    let head: GitSha = run(args, &signer_environment)?
        .parse()
        .map_err(|_| PublicationError::VerificationFailed)?;
    let object = head.to_string();
    let loose_object = format!("objects/{}/{}", &object[..2], &object[2..]);
    // An idempotent retry can reuse an already packed commit. A newly written
    // loose object must be readable by the next credential-free worker.
    if spec
        .worktree()
        .join(".git")
        .join(&loose_object)
        .try_exists()
        .map_err(io_error)?
    {
        crate::isolated_workspace::share_git_metadata_path(
            spec.worktree(),
            Path::new(&loose_object),
            0o440,
        )
        .map_err(io_error)?;
    }
    let mut verify = signing_args.clone();
    verify.extend(["verify-commit".into(), head.to_string()]);
    run(verify, &plain)?;
    let mut fingerprint_args = signing_args;
    fingerprint_args.extend([
        "show".into(),
        "-s".into(),
        "--format=%GF".into(),
        head.to_string(),
    ]);
    let fingerprint = run(fingerprint_args, &plain)?;
    if !fingerprint.starts_with("SHA256:") || fingerprint.contains(['\r', '\n']) {
        return Err(PublicationError::VerificationFailed);
    }
    let actual = run(
        vec![
            "show".into(),
            "-s".into(),
            "--format=%T%n%P%n%an%n%ae%n%cn%n%ce%n%ct".into(),
            head.to_string(),
        ],
        &plain,
    )?;
    let expected = format!(
        "{tree}\n{parent}\n{}\n{}\n{}\n{}\n{timestamp}",
        identity.name, identity.email, identity.name, identity.email
    );
    // Publication may already have aligned the branch before a crash. Accept
    // only the original source or this exact, independently recreated commit.
    if current_head != source && current_head != head.to_string() {
        return Err(PublicationError::LocalHeadDrift);
    }
    if actual != expected || run(vec!["rev-parse".into(), "HEAD".into()], &plain)? != current_head {
        return Err(PublicationError::VerificationFailed);
    }
    Ok(SignedCommit {
        source_head: spec.local_head(),
        tree,
        head,
        parent,
        signer_fingerprint: fingerprint,
    })
}

fn io_error(error: std::io::Error) -> PublicationError {
    PublicationError::Filesystem(error.to_string())
}
