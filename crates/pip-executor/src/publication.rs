//! Race-safe publication of controller-owned Git branches.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use pip_core::GitSha;

use crate::{GitCommand, GitRunner};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitPublicationSpec {
    worktree: PathBuf,
    remote: String,
    branch: String,
    local_head: GitSha,
    expected_remote_head: Option<GitSha>,
}

impl GitPublicationSpec {
    pub fn new(
        worktree: impl AsRef<Path>,
        remote: impl Into<String>,
        branch: impl Into<String>,
        local_head: GitSha,
        expected_remote_head: Option<GitSha>,
    ) -> Result<Self, PublicationError> {
        let input = worktree.as_ref();
        if fs::symlink_metadata(input)
            .map_err(|error| PublicationError::Filesystem(error.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(PublicationError::InvalidSpec);
        }
        let worktree = input
            .canonicalize()
            .map_err(|error| PublicationError::Filesystem(error.to_string()))?;
        if !worktree.is_dir() {
            return Err(PublicationError::InvalidSpec);
        }
        let remote = remote.into();
        let branch = branch.into();
        if !valid_remote(&remote) || !valid_owned_branch(&branch) {
            return Err(PublicationError::InvalidSpec);
        }
        Ok(Self {
            worktree,
            remote,
            branch,
            local_head,
            expected_remote_head,
        })
    }

    #[must_use]
    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    #[must_use]
    pub fn remote(&self) -> &str {
        &self.remote
    }

    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    #[must_use]
    pub const fn local_head(&self) -> GitSha {
        self.local_head
    }

    #[must_use]
    pub const fn expected_remote_head(&self) -> Option<GitSha> {
        self.expected_remote_head
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationResult {
    Created,
    Updated,
    Existing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicationError {
    InvalidConfiguration,
    InvalidSpec,
    DirtyWorktree,
    LocalHeadDrift,
    BranchDrift,
    RemoteRace,
    VerificationFailed,
    TimedOut,
    OutputTooLarge,
    CommandFailed(i32),
    MalformedOutput(String),
    Filesystem(String),
    Process(String),
}

impl fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid Git publisher configuration")
            }
            Self::InvalidSpec => formatter.write_str("invalid Git publication spec"),
            Self::DirtyWorktree => formatter.write_str("Git worktree has uncommitted changes"),
            Self::LocalHeadDrift => {
                formatter.write_str("local Git head differs from the bound head")
            }
            Self::BranchDrift => formatter.write_str("worktree is not on the bound Pip branch"),
            Self::RemoteRace => {
                formatter.write_str("remote Git branch changed outside the transaction")
            }
            Self::VerificationFailed => {
                formatter.write_str("published Git branch could not be verified")
            }
            Self::TimedOut => formatter.write_str("Git publication command timed out"),
            Self::OutputTooLarge => {
                formatter.write_str("Git publication output exceeded its bound")
            }
            Self::CommandFailed(status) => {
                write!(formatter, "Git publication command exited with {status}")
            }
            Self::MalformedOutput(error) => {
                write!(formatter, "malformed Git publication output: {error}")
            }
            Self::Filesystem(error) => {
                write!(formatter, "Git publication filesystem error: {error}")
            }
            Self::Process(error) => write!(formatter, "Git publication process failed: {error}"),
        }
    }
}

impl std::error::Error for PublicationError {}

pub struct GitPublisher<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: GitRunner> GitPublisher<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, PublicationError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(PublicationError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            timeout,
            max_output_bytes,
        })
    }

    pub fn publish(
        &self,
        spec: &GitPublicationSpec,
    ) -> Result<PublicationResult, PublicationError> {
        let local_head = self.single_line(spec, ["rev-parse", "--verify", "HEAD^{commit}"])?;
        let local_head = GitSha::from_str(&local_head)
            .map_err(|error| PublicationError::MalformedOutput(error.to_string()))?;
        if local_head != spec.local_head() {
            return Err(PublicationError::LocalHeadDrift);
        }

        let branch = self.single_line(spec, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if branch != spec.branch() {
            return Err(PublicationError::BranchDrift);
        }
        let status = self.execute(
            spec,
            ["status", "--porcelain=v1", "-z"]
                .map(str::to_owned)
                .to_vec(),
        )?;
        if !status.stdout.is_empty() {
            return Err(PublicationError::DirtyWorktree);
        }

        let actual_remote = self.remote_head(spec)?;
        if actual_remote == Some(spec.local_head()) {
            return Ok(PublicationResult::Existing);
        }
        if actual_remote != spec.expected_remote_head() {
            return Err(PublicationError::RemoteRace);
        }

        let remote_ref = format!("refs/heads/{}", spec.branch());
        let expected = spec
            .expected_remote_head()
            .map(|head| head.to_string())
            .unwrap_or_default();
        self.execute(
            spec,
            vec![
                "push".into(),
                "--porcelain".into(),
                format!("--force-with-lease={remote_ref}:{expected}"),
                spec.remote().into(),
                format!("HEAD:{remote_ref}"),
            ],
        )?;
        if self.remote_head(spec)? != Some(spec.local_head()) {
            return Err(PublicationError::VerificationFailed);
        }
        Ok(if spec.expected_remote_head().is_some() {
            PublicationResult::Updated
        } else {
            PublicationResult::Created
        })
    }

    fn remote_head(&self, spec: &GitPublicationSpec) -> Result<Option<GitSha>, PublicationError> {
        let remote_ref = format!("refs/heads/{}", spec.branch());
        let output = self.execute(
            spec,
            vec![
                "ls-remote".into(),
                "--heads".into(),
                spec.remote().into(),
                remote_ref.clone(),
            ],
        )?;
        if output.stdout.is_empty() {
            return Ok(None);
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|error| PublicationError::MalformedOutput(error.to_string()))?;
        let lines = text.lines().collect::<Vec<_>>();
        let [line] = lines.as_slice() else {
            return Err(PublicationError::MalformedOutput(
                "expected at most one remote ref".into(),
            ));
        };
        let (sha, reference) = line.split_once('\t').ok_or_else(|| {
            PublicationError::MalformedOutput("remote ref lacks a tab separator".into())
        })?;
        if reference != remote_ref {
            return Err(PublicationError::MalformedOutput(
                "remote returned an unexpected ref".into(),
            ));
        }
        GitSha::from_str(sha)
            .map(Some)
            .map_err(|error| PublicationError::MalformedOutput(error.to_string()))
    }

    fn single_line<const N: usize>(
        &self,
        spec: &GitPublicationSpec,
        args: [&str; N],
    ) -> Result<String, PublicationError> {
        let output = self.execute(spec, args.map(str::to_owned).to_vec())?;
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|error| PublicationError::MalformedOutput(error.to_string()))?;
        let value = text.trim_end_matches(['\r', '\n']);
        if value.is_empty() || value.contains(['\r', '\n']) {
            return Err(PublicationError::MalformedOutput(
                "expected exactly one output line".into(),
            ));
        }
        Ok(value.to_owned())
    }

    fn execute(
        &self,
        spec: &GitPublicationSpec,
        args: Vec<String>,
    ) -> Result<crate::GitOutput, PublicationError> {
        let output = self
            .runner
            .run(&GitCommand {
                program: self.program.clone(),
                cwd: spec.worktree().to_owned(),
                args,
                timeout: self.timeout,
                max_output_bytes: self.max_output_bytes,
            })
            .map_err(|error| PublicationError::Process(error.to_string()))?;
        if output.timed_out {
            return Err(PublicationError::TimedOut);
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(PublicationError::OutputTooLarge);
        }
        if output.status != 0 {
            return Err(PublicationError::CommandFailed(output.status));
        }
        Ok(output)
    }
}

fn valid_remote(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_owned_branch(value: &str) -> bool {
    value.starts_with("pip/")
        && value.len() <= 255
        && !value.ends_with('/')
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("//")
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
}
