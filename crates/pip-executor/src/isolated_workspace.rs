//! Case-local Git storage shared by the controller and credential-free worker.
//!
//! The private fetch cache is never a worker's Git common directory or object
//! alternate. Legacy linked worktrees are deliberately not converted in place.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use crate::{
    AllocationError, AllocationResult, GitCommand, GitRunner, RetirementResult,
    WorktreeRetirementSpec, WorktreeSpec,
};

pub struct IsolatedWorkspace<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

/// Trust only this already-bound workspace across the two service UIDs. These
/// overrides reach Git subprocesses launched by the provider as well as Pip.
/// Never write a wildcard safe.directory into a shared/global Git config.
pub fn workspace_git_environment(
    path: &Path,
    mut environment: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    environment.retain(|key, _| !key.starts_with("GIT_CONFIG_"));
    environment.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
    environment.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
    // Read-only probes must not rewrite a worker's index under the controller's
    // private umask. Explicit add/commit operations still acquire normal locks.
    environment.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
    let values = [
        ("safe.directory", path.to_string_lossy().into_owned()),
        ("core.hooksPath", "/dev/null".into()),
        ("core.fsmonitor", "false".into()),
        ("credential.helper", String::new()),
    ];
    environment.insert("GIT_CONFIG_COUNT".into(), values.len().to_string());
    for (index, (key, value)) in values.into_iter().enumerate() {
        environment.insert(format!("GIT_CONFIG_KEY_{index}"), key.into());
        environment.insert(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    environment
}

impl<R: GitRunner> IsolatedWorkspace<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, AllocationError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(AllocationError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            timeout,
            max_output_bytes,
        })
    }

    pub fn allocate(
        &self,
        spec: &WorktreeSpec,
        remote_url: &str,
    ) -> Result<AllocationResult, AllocationError> {
        if !remote_url.starts_with("https://")
            || remote_url.chars().any(char::is_whitespace)
            || remote_url.len() > 4096
        {
            return Err(AllocationError::InvalidSpec);
        }
        if fs::symlink_metadata(spec.path()).is_ok() {
            self.verify_for_controller(spec.path(), spec.branch(), remote_url)?;
            return Ok(AllocationResult::Existing);
        }
        // Initialize a fresh Git directory, not a copy of the cache's config,
        // hooks, credentials, hard-linked objects, or alternate object stores.
        let temporary = tempfile::Builder::new()
            .prefix(".pip-prepare-")
            .tempdir_in(spec.root())
            .map_err(io_error)?;
        let path = temporary.path();
        self.git(path, &["init", "--quiet", "--template="])?;
        self.git(
            path,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--",
                spec.repository()
                    .to_str()
                    .ok_or(AllocationError::InvalidSpec)?,
                &spec.base().to_string(),
            ],
        )?;
        self.git(path, &["remote", "add", "origin", remote_url])?;
        self.git(
            path,
            &[
                "checkout",
                "--quiet",
                "--no-track",
                "-b",
                spec.branch(),
                &spec.base().to_string(),
            ],
        )?;
        fs::write(path.join(".git/pip-case-branch"), spec.branch()).map_err(io_error)?;
        self.verify(path, spec.branch())?;
        share_created_tree(path)?;
        if fs::symlink_metadata(spec.path()).is_ok() {
            return Err(AllocationError::Collision);
        }
        fs::rename(path, spec.path()).map_err(io_error)?;
        // The temporary directory no longer exists; its guard cannot touch the
        // published checkout. Incomplete preparation is never dispatched.
        Ok(AllocationResult::Created)
    }

    /// Called before a credential-bearing publisher receives this repository.
    /// The caller first binds the path, branch and URL to authoritative policy.
    pub fn verify_for_controller(
        &self,
        path: &Path,
        branch: &str,
        remote_url: &str,
    ) -> Result<(), AllocationError> {
        self.verify(path, branch)?;
        if self.git(path, &["remote", "get-url", "--all", "origin"])? != remote_url {
            return Err(AllocationError::VerificationFailed);
        }
        Ok(())
    }

    pub fn retire(
        &self,
        spec: &WorktreeRetirementSpec,
    ) -> Result<RetirementResult, AllocationError> {
        if fs::symlink_metadata(spec.path())
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(RetirementResult::Absent);
        }
        self.verify(spec.path(), spec.branch())?;
        if !self
            .git(
                spec.path(),
                &["status", "--porcelain=v1", "--untracked-files=all"],
            )?
            .is_empty()
        {
            return Err(AllocationError::DirtyWorktree);
        }
        self.retain_source_builds(spec)?;
        // Retain the exact local branch before reclaiming its source tree,
        // just as linked-worktree retirement retains the branch in the cache.
        // Never force over an independently changed controller-side branch.
        let head = self.git(spec.path(), &["rev-parse", "HEAD"])?;
        let branch_ref = format!("refs/heads/{}", spec.branch());
        let refspec = format!("{branch_ref}:{branch_ref}");
        self.git(
            spec.repository(),
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--",
                spec.path().to_str().ok_or(AllocationError::InvalidSpec)?,
                &refspec,
            ],
        )?;
        if self.git(spec.repository(), &["rev-parse", "--verify", &branch_ref])? != head {
            return Err(AllocationError::VerificationFailed);
        }
        // Rust's directory removal does not follow symlinks. The root, local
        // metadata, branch, and clean state have all been bound above.
        fs::remove_dir_all(spec.path()).map_err(io_error)?;
        Ok(RetirementResult::Retired)
    }

    fn retain_source_builds(&self, spec: &WorktreeRetirementSpec) -> Result<(), AllocationError> {
        let sources = self.git(
            spec.path(),
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/pip/source-builds/",
            ],
        )?;
        for line in sources.lines() {
            let (reference, source) = line
                .split_once(' ')
                .ok_or(AllocationError::VerificationFailed)?;
            if source.parse::<pip_core::GitSha>().is_err()
                || reference != format!("refs/pip/source-builds/{source}")
                || self.git(
                    spec.path(),
                    &["--no-replace-objects", "cat-file", "-t", source],
                )? != "commit"
            {
                return Err(AllocationError::VerificationFailed);
            }
            let previous = self.git(
                spec.repository(),
                &["for-each-ref", "--format=%(objectname)", reference],
            )?;
            if !previous.is_empty() && previous != source {
                return Err(AllocationError::VerificationFailed);
            }
            // Fetch immutable objects, then create the retention ref with CAS.
            // Fetch alone can overwrite non-head namespaces without --force.
            self.git(
                spec.repository(),
                &[
                    "-c",
                    "fetch.fsckObjects=true",
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--",
                    spec.path().to_str().ok_or(AllocationError::InvalidSpec)?,
                    source,
                ],
            )?;
            let expected = if previous.is_empty() {
                "0".repeat(40)
            } else {
                previous
            };
            self.git(
                spec.repository(),
                &["update-ref", reference, source, &expected],
            )?;
            if self.git(spec.repository(), &["rev-parse", "--verify", reference])? != source {
                return Err(AllocationError::VerificationFailed);
            }
        }
        Ok(())
    }

    fn verify(&self, path: &Path, branch: &str) -> Result<(), AllocationError> {
        let root = fs::symlink_metadata(path).map_err(io_error)?;
        let metadata = path.join(".git");
        if !root.is_dir()
            || root.file_type().is_symlink()
            || !fs::symlink_metadata(&metadata).map_err(io_error)?.is_dir()
        {
            return Err(AllocationError::Collision);
        }
        validate_metadata_tree(&metadata)?;
        for forbidden in [
            "commondir",
            "objects/info/alternates",
            "objects/info/http-alternates",
            "worktrees",
        ] {
            if fs::symlink_metadata(metadata.join(forbidden)).is_ok() {
                return Err(AllocationError::VerificationFailed);
            }
        }
        // Read configuration without following includes, before status,
        // checkout, fetch or publication can invoke repository-defined tools.
        // A positive allowlist also rejects clean/process filters, URL
        // rewrites, upload-pack hooks and future unknown executable settings.
        let config = metadata.join("config");
        if fs::metadata(&config).map_err(io_error)?.len() > 65_536 {
            return Err(AllocationError::VerificationFailed);
        }
        let configuration = self.git(
            path,
            &[
                "config",
                "--file",
                config.to_str().ok_or(AllocationError::InvalidSpec)?,
                "--no-includes",
                "--null",
                "--list",
            ],
        )?;
        for entry in configuration.split_terminator('\0') {
            let (key, value) = entry
                .split_once('\n')
                .ok_or(AllocationError::VerificationFailed)?;
            if !safe_case_config(key, value) {
                return Err(AllocationError::VerificationFailed);
            }
        }
        if fs::read_to_string(metadata.join("pip-case-branch")).map_err(io_error)? != branch
            || self.git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])? != branch
            || Path::new(&self.git(path, &["rev-parse", "--show-toplevel"])?)
                .canonicalize()
                .map_err(io_error)?
                != path.canonicalize().map_err(io_error)?
            || Path::new(&self.git(path, &["rev-parse", "--absolute-git-dir"])?)
                .canonicalize()
                .map_err(io_error)?
                != metadata.canonicalize().map_err(io_error)?
        {
            return Err(AllocationError::VerificationFailed);
        }
        Ok(())
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Result<String, AllocationError> {
        let output = self.runner.run(&GitCommand {
            program: self.program.clone(),
            cwd: cwd.to_owned(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
            environment: workspace_git_environment(cwd, BTreeMap::new()),
        })?;
        if output.timed_out {
            return Err(AllocationError::TimedOut);
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(AllocationError::OutputTooLarge);
        }
        if output.status != 0 {
            return Err(AllocationError::CommandFailed(output.status));
        }
        String::from_utf8(output.stdout)
            .map(|value| value.trim_end_matches('\n').to_owned())
            .map_err(|error| AllocationError::MalformedOutput(error.to_string()))
    }
}

fn safe_case_config(key: &str, value: &str) -> bool {
    match key {
        "core.repositoryformatversion" => value == "0",
        "core.sharedrepository" => value == "1",
        "core.bare" | "core.fsmonitor" | "commit.gpgsign" => value == "false",
        "core.hookspath" => value == "/dev/null",
        "core.filemode"
        | "core.logallrefupdates"
        | "core.ignorecase"
        | "core.precomposeunicode"
        | "core.symlinks"
        | "receive.denynonfastforwards" => matches!(value, "true" | "false"),
        "core.autocrlf" => matches!(value, "true" | "false" | "input"),
        "core.eol" => matches!(value, "lf" | "crlf" | "native"),
        "remote.origin.url" => {
            value.starts_with("https://")
                && !value
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '@')
        }
        "remote.origin.fetch" => value == "+refs/heads/*:refs/remotes/origin/*",
        "user.name" | "user.email" => value.len() <= 256 && !value.chars().any(char::is_control),
        _ => false,
    }
}

fn io_error(error: std::io::Error) -> AllocationError {
    AllocationError::Filesystem(error.to_string())
}

fn validate_metadata_tree(path: &Path) -> Result<(), AllocationError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || (!metadata.is_dir() && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(AllocationError::VerificationFailed);
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(io_error)? {
            validate_metadata_tree(&entry.map_err(io_error)?.path())?;
        }
    }
    Ok(())
}

fn share_created_tree(path: &Path) -> Result<(), AllocationError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    let mode = if metadata.is_dir() {
        // Both service identities have the same primary group. Setgid is
        // unnecessary and intentionally forbidden by their systemd sandboxes.
        0o770
    } else if metadata.is_file() {
        if metadata.permissions().mode() & 0o111 != 0 {
            0o770
        } else {
            0o660
        }
    } else {
        return Err(AllocationError::VerificationFailed);
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(io_error)? {
            share_created_tree(&entry.map_err(io_error)?.path())?;
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io_error)
}

/// Share only a controller-written Git object/ref/log and its directories.
/// Existing worker-owned entries with sufficient access need no chmod. Never
/// traverse a symlink, chmod a hard link, or change the process-wide umask.
pub(crate) fn share_git_metadata_path(
    worktree: &Path,
    relative: &Path,
    file_mode: u32,
) -> Result<(), std::io::Error> {
    let mut path = worktree.join(".git");
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            let std::path::Component::Normal(name) = component else {
                return Err(std::io::Error::other("invalid Git metadata path"));
            };
            path.push(name);
        }
        let metadata = fs::symlink_metadata(&path)?;
        let required = if metadata.is_dir() {
            0o770
        } else if metadata.is_file() && metadata.nlink() == 1 {
            file_mode
        } else {
            return Err(std::io::Error::other("unsafe Git metadata path"));
        };
        if metadata.permissions().mode() & required != required {
            fs::set_permissions(&path, fs::Permissions::from_mode(required))?;
        }
    }
    Ok(())
}
