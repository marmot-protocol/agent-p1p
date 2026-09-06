//! Hardened reconciliation of an existing canonical repository checkout.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use pip_core::GitSha;

use crate::{BoundedProcessRunner, ProcessError, ProcessOutput, ProcessRunner, ProcessSpec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckoutError {
    InvalidConfiguration,
    InvalidCheckout,
    InvalidRemote,
    InvalidBranch,
    RemoteDrift,
    Process(ProcessError),
    TimedOut,
    OutputTooLarge,
    CommandFailed(i32),
    InvalidUtf8,
    InvalidHead,
    DirtyWorktree,
    BranchDrift,
    HeadDrift,
}

impl fmt::Display for CheckoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid checkout reconciler configuration")
            }
            Self::InvalidCheckout => formatter.write_str("canonical checkout is invalid"),
            Self::InvalidRemote => formatter.write_str("canonical checkout remote is invalid"),
            Self::InvalidBranch => formatter.write_str("default branch is invalid"),
            Self::RemoteDrift => formatter.write_str("canonical checkout remote URL drifted"),
            Self::Process(error) => error.fmt(formatter),
            Self::TimedOut => formatter.write_str("checkout reconciliation timed out"),
            Self::OutputTooLarge => {
                formatter.write_str("checkout reconciliation output exceeded its bound")
            }
            Self::CommandFailed(status) => {
                write!(formatter, "Git checkout command exited with {status}")
            }
            Self::InvalidUtf8 => formatter.write_str("Git checkout output is not UTF-8"),
            Self::InvalidHead => {
                formatter.write_str("default branch did not resolve to an exact commit")
            }
            Self::DirtyWorktree => formatter.write_str("case worktree is dirty"),
            Self::BranchDrift => formatter.write_str("case worktree branch drifted"),
            Self::HeadDrift => formatter.write_str("case worktree head drifted"),
        }
    }
}

impl std::error::Error for CheckoutError {}

impl From<ProcessError> for CheckoutError {
    fn from(error: ProcessError) -> Self {
        Self::Process(error)
    }
}

pub struct CheckoutReconciler<R> {
    runner: R,
    program: String,
    environment: BTreeMap<String, String>,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: ProcessRunner> CheckoutReconciler<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        mut environment: BTreeMap<String, String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, CheckoutError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(CheckoutError::InvalidConfiguration);
        }
        environment.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
        environment.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        environment.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        Ok(Self {
            runner,
            program,
            environment,
            timeout,
            max_output_bytes,
        })
    }

    pub fn fetch_default_head(
        &self,
        checkout: impl AsRef<Path>,
        expected_remote_url: &str,
        default_branch: &str,
    ) -> Result<GitSha, CheckoutError> {
        let checkout = canonical_directory(checkout.as_ref())?;
        if !valid_remote(expected_remote_url) {
            return Err(CheckoutError::InvalidRemote);
        }
        if !valid_branch(default_branch) {
            return Err(CheckoutError::InvalidBranch);
        }
        let top = self.text(
            &checkout,
            vec!["rev-parse".into(), "--show-toplevel".into()],
        )?;
        let top = Path::new(top.trim())
            .canonicalize()
            .map_err(|_| CheckoutError::InvalidCheckout)?;
        if top != checkout {
            return Err(CheckoutError::InvalidCheckout);
        }
        let remotes = self.text(
            &checkout,
            vec![
                "remote".into(),
                "get-url".into(),
                "--all".into(),
                "origin".into(),
            ],
        )?;
        let remotes = remotes
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        if remotes != [expected_remote_url] {
            return Err(CheckoutError::RemoteDrift);
        }
        let remote_ref = format!("refs/remotes/origin/{default_branch}");
        self.checked(
            &checkout,
            vec![
                "fetch".into(),
                "--no-tags".into(),
                "--prune".into(),
                "--force".into(),
                expected_remote_url.into(),
                format!("+refs/heads/{default_branch}:{remote_ref}"),
            ],
        )?;
        let head = self.text(
            &checkout,
            vec!["rev-parse".into(), "--verify".into(), remote_ref],
        )?;
        let head = GitSha::from_str(head.trim()).map_err(|_| CheckoutError::InvalidHead)?;
        self.checked(
            &checkout,
            vec!["cat-file".into(), "-e".into(), format!("{head}^{{commit}}")],
        )?;
        Ok(head)
    }

    pub fn verify_worktree(
        &self,
        worktree: impl AsRef<Path>,
        expected_branch: &str,
        expected_head: GitSha,
    ) -> Result<(), CheckoutError> {
        let worktree = canonical_directory(worktree.as_ref())?;
        if !valid_branch(expected_branch) {
            return Err(CheckoutError::InvalidBranch);
        }
        let branch = self.text(
            &worktree,
            vec![
                "symbolic-ref".into(),
                "--quiet".into(),
                "--short".into(),
                "HEAD".into(),
            ],
        )?;
        if branch.trim() != expected_branch {
            return Err(CheckoutError::BranchDrift);
        }
        let head = self.text(
            &worktree,
            vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
        )?;
        if head.trim() != expected_head.to_string() {
            return Err(CheckoutError::HeadDrift);
        }
        let status = self.text(
            &worktree,
            vec![
                "status".into(),
                "--porcelain=v1".into(),
                "--untracked-files=all".into(),
            ],
        )?;
        if !status.is_empty() {
            return Err(CheckoutError::DirtyWorktree);
        }
        Ok(())
    }

    fn text(&self, cwd: &Path, args: Vec<String>) -> Result<String, CheckoutError> {
        let output = self.checked(cwd, args)?;
        String::from_utf8(output.stdout).map_err(|_| CheckoutError::InvalidUtf8)
    }

    fn checked(&self, cwd: &Path, args: Vec<String>) -> Result<ProcessOutput, CheckoutError> {
        let mut hardened = vec![
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "core.fsmonitor=false".into(),
            "-c".into(),
            "credential.helper=".into(),
            "-c".into(),
            "http.proxy=".into(),
            "-c".into(),
            "http.extraHeader=".into(),
            "-c".into(),
            "http.sslVerify=true".into(),
            "-c".into(),
            "protocol.file.allow=never".into(),
        ];
        hardened.extend(args);
        let output = self.runner.run(&ProcessSpec {
            program: self.program.clone(),
            args: hardened,
            stdin_file: None,
            cwd: cwd.to_owned(),
            environment: self.environment.clone(),
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        if output.timed_out {
            return Err(CheckoutError::TimedOut);
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(CheckoutError::OutputTooLarge);
        }
        if output.status != 0 {
            return Err(CheckoutError::CommandFailed(output.status));
        }
        Ok(output)
    }
}

impl CheckoutReconciler<BoundedProcessRunner> {
    pub fn process(
        program: impl Into<String>,
        environment: BTreeMap<String, String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, CheckoutError> {
        Self::new(
            BoundedProcessRunner,
            program,
            environment,
            timeout,
            max_output_bytes,
        )
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, CheckoutError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| CheckoutError::InvalidCheckout)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CheckoutError::InvalidCheckout);
    }
    path.canonicalize()
        .map_err(|_| CheckoutError::InvalidCheckout)
}

fn valid_remote(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.trim() == value
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value != "."
        && value != ".."
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}
