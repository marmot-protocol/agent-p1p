//! Deterministic worktree allocation.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use pip_core::{CaseId, GitSha};
use wait_timeout::ChildExt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommand {
    pub program: String,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub environment: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

pub trait GitRunner {
    fn run(&self, command: &GitCommand) -> Result<GitOutput, AllocationError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessGitRunner;

impl GitRunner for ProcessGitRunner {
    fn run(&self, command: &GitCommand) -> Result<GitOutput, AllocationError> {
        let mut args = vec![
            "-c".into(),
            "core.hooksPath=/dev/null".into(),
            "-c".into(),
            "credential.helper=".into(),
            "-c".into(),
            "http.proxy=".into(),
            "-c".into(),
            "http.extraHeader=".into(),
            "-c".into(),
            "http.sslVerify=true".into(),
        ];
        args.extend(command.args.iter().cloned());
        let mut environment = crate::sanitized_environment();
        environment.extend(command.environment.clone());
        environment.insert("GIT_CONFIG_GLOBAL".into(), "/dev/null".into());
        environment.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        environment.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        let mut child = Command::new(&command.program)
            .args(args)
            .current_dir(&command.cwd)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| AllocationError::Process(error.to_string()))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AllocationError::Process("failed to capture Git standard output".into())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AllocationError::Process("failed to capture Git standard error".into())
        })?;
        let limit = command.max_output_bytes.saturating_add(1);
        let stdout_reader = thread::spawn(move || drain_bounded(stdout, limit));
        let stderr_reader = thread::spawn(move || drain_bounded(stderr, limit));
        let status = child
            .wait_timeout(command.timeout)
            .map_err(|error| AllocationError::Process(error.to_string()))?;
        let timed_out = status.is_none();
        let status = if let Some(status) = status {
            status
        } else {
            child
                .kill()
                .map_err(|error| AllocationError::Process(error.to_string()))?;
            child
                .wait()
                .map_err(|error| AllocationError::Process(error.to_string()))?
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| AllocationError::Process("Git stdout reader panicked".into()))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| AllocationError::Process("Git stderr reader panicked".into()))??;
        Ok(GitOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
            timed_out,
        })
    }
}

fn drain_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, AllocationError> {
    let mut retained = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| AllocationError::Process(error.to_string()))?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocationError {
    InvalidConfiguration,
    InvalidSpec,
    Collision,
    TimedOut,
    OutputTooLarge,
    CommandFailed(i32),
    MalformedOutput(String),
    VerificationFailed,
    DirtyWorktree,
    Filesystem(String),
    Process(String),
}

impl fmt::Display for AllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid Git allocator configuration")
            }
            Self::InvalidSpec => formatter.write_str("invalid worktree allocation spec"),
            Self::Collision => formatter.write_str("worktree path or branch is already owned"),
            Self::TimedOut => formatter.write_str("Git command timed out"),
            Self::OutputTooLarge => formatter.write_str("Git output exceeded its configured bound"),
            Self::CommandFailed(status) => write!(formatter, "Git command exited with {status}"),
            Self::MalformedOutput(error) => write!(formatter, "malformed Git output: {error}"),
            Self::VerificationFailed => {
                formatter.write_str("created worktree could not be verified")
            }
            Self::DirtyWorktree => {
                formatter.write_str("managed worktree contains uncommitted changes")
            }
            Self::Filesystem(error) => write!(formatter, "worktree filesystem error: {error}"),
            Self::Process(error) => write!(formatter, "Git process failed: {error}"),
        }
    }
}

impl std::error::Error for AllocationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeSpec {
    repository: PathBuf,
    root: PathBuf,
    path: PathBuf,
    branch: String,
    base: GitSha,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeRetirementSpec {
    repository: PathBuf,
    path: PathBuf,
    branch: String,
}

impl WorktreeSpec {
    pub fn new(
        repository: impl AsRef<Path>,
        root: impl AsRef<Path>,
        branch_prefix: &str,
        case_id: CaseId,
        base: GitSha,
    ) -> Result<Self, AllocationError> {
        let repository = canonical_directory(repository.as_ref())?;
        let (root, path, branch) = managed_identity(root.as_ref(), branch_prefix, case_id)?;
        Ok(Self {
            repository,
            root,
            path,
            branch,
            base,
        })
    }

    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }

    #[must_use]
    pub const fn base(&self) -> GitSha {
        self.base
    }

    #[must_use]
    pub fn retirement_spec(&self) -> WorktreeRetirementSpec {
        WorktreeRetirementSpec {
            repository: self.repository.clone(),
            path: self.path.clone(),
            branch: self.branch.clone(),
        }
    }
}

impl WorktreeRetirementSpec {
    pub fn new(
        repository: impl AsRef<Path>,
        root: impl AsRef<Path>,
        branch_prefix: &str,
        case_id: CaseId,
    ) -> Result<Self, AllocationError> {
        let repository = canonical_directory(repository.as_ref())?;
        let (_, path, branch) = managed_identity(root.as_ref(), branch_prefix, case_id)?;
        Ok(Self {
            repository,
            path,
            branch,
        })
    }

    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn branch(&self) -> &str {
        &self.branch
    }
}

fn managed_identity(
    root: &Path,
    branch_prefix: &str,
    case_id: CaseId,
) -> Result<(PathBuf, PathBuf, String), AllocationError> {
    if !valid_branch_prefix(branch_prefix) {
        return Err(AllocationError::InvalidSpec);
    }
    if fs::symlink_metadata(root)
        .map_err(|error| AllocationError::Filesystem(error.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(AllocationError::InvalidSpec);
    }
    let root = canonical_directory(root)?;
    let identity = format!(
        "repo-{}-issue-{}-workflow-{}",
        case_id.repository().get(),
        case_id.issue().get(),
        case_id.workflow().get()
    );
    let branch = format!(
        "{branch_prefix}repo-{}/issue-{}/workflow-{}",
        case_id.repository().get(),
        case_id.issue().get(),
        case_id.workflow().get()
    );
    let path = root.join(identity);
    if !path.starts_with(&root) {
        return Err(AllocationError::InvalidSpec);
    }
    Ok((root, path, branch))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, AllocationError> {
    if !path.is_dir() {
        return Err(AllocationError::InvalidSpec);
    }
    path.canonicalize()
        .map_err(|error| AllocationError::Filesystem(error.to_string()))
}

fn valid_branch_prefix(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.ends_with('/')
        && value
            .strip_suffix('/')
            .is_some_and(|prefix| prefix.split('/').all(valid_segment))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocationResult {
    Created,
    Existing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetirementResult {
    Retired,
    Absent,
}

pub struct WorktreeAllocator<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

pub struct WorktreeRetirer<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: GitRunner> WorktreeRetirer<R> {
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

    pub fn retire(
        &self,
        spec: &WorktreeRetirementSpec,
    ) -> Result<RetirementResult, AllocationError> {
        let repository = self.execute_at(
            spec.repository(),
            vec!["rev-parse".into(), "--show-toplevel".into()],
        )?;
        let reported = std::str::from_utf8(&repository.stdout)
            .map_err(|error| AllocationError::MalformedOutput(error.to_string()))?
            .trim();
        let reported = Path::new(reported)
            .canonicalize()
            .map_err(|error| AllocationError::Filesystem(error.to_string()))?;
        if reported != spec.repository() {
            return Err(AllocationError::VerificationFailed);
        }

        let records = self.worktrees(spec)?;
        match classify_retirement_existing(spec, &records)? {
            None => {
                if fs::symlink_metadata(spec.path()).is_ok() {
                    return Err(AllocationError::Collision);
                }
                return Ok(RetirementResult::Absent);
            }
            Some(AllocationResult::Existing) => {}
            Some(AllocationResult::Created) => unreachable!("classification never creates"),
        }
        if fs::symlink_metadata(spec.path())
            .map_err(|error| AllocationError::Filesystem(error.to_string()))?
            .file_type()
            .is_symlink()
        {
            return Err(AllocationError::Collision);
        }
        let status = self.execute_at(
            spec.path(),
            vec![
                "status".into(),
                "--porcelain=v1".into(),
                "--untracked-files=all".into(),
            ],
        )?;
        if !status.stdout.is_empty() {
            return Err(AllocationError::DirtyWorktree);
        }
        self.execute_at(
            spec.repository(),
            vec![
                "worktree".into(),
                "remove".into(),
                "--".into(),
                spec.path().to_string_lossy().into_owned(),
            ],
        )?;
        if classify_retirement_existing(spec, &self.worktrees(spec)?)?.is_some()
            || spec.path().exists()
        {
            return Err(AllocationError::VerificationFailed);
        }
        Ok(RetirementResult::Retired)
    }

    fn worktrees(
        &self,
        spec: &WorktreeRetirementSpec,
    ) -> Result<Vec<WorktreeRecord>, AllocationError> {
        let output = self.execute_at(
            spec.repository(),
            vec![
                "worktree".into(),
                "list".into(),
                "--porcelain".into(),
                "-z".into(),
            ],
        )?;
        parse_worktrees(&output.stdout)
    }

    fn execute_at(&self, cwd: &Path, args: Vec<String>) -> Result<GitOutput, AllocationError> {
        let output = self.runner.run(&GitCommand {
            program: self.program.clone(),
            cwd: cwd.to_owned(),
            args,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
            environment: BTreeMap::new(),
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
        Ok(output)
    }
}

impl<R: GitRunner> WorktreeAllocator<R> {
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

    pub fn allocate(&self, spec: &WorktreeSpec) -> Result<AllocationResult, AllocationError> {
        self.verify_repository(spec)?;
        let records = self.worktrees(spec)?;
        if let Some(result) = classify_existing(spec, &records)? {
            return Ok(result);
        }
        if fs::symlink_metadata(spec.path()).is_ok() {
            return Err(AllocationError::Collision);
        }

        let branch_ref = format!("refs/heads/{}", spec.branch());
        let branch = self.raw(
            spec,
            vec![
                "show-ref".into(),
                "--verify".into(),
                "--quiet".into(),
                branch_ref,
            ],
        )?;
        let mut args = vec!["worktree".into(), "add".into(), "--no-track".into()];
        match branch.status {
            0 => {
                args.extend([
                    spec.path().to_string_lossy().into_owned(),
                    spec.branch().into(),
                ]);
            }
            1 => {
                args.extend([
                    "-b".into(),
                    spec.branch().into(),
                    spec.path().to_string_lossy().into_owned(),
                    spec.base().to_string(),
                ]);
            }
            status => return Err(AllocationError::CommandFailed(status)),
        }
        self.execute(spec, args)?;
        let records = self.worktrees(spec)?;
        if classify_existing(spec, &records)? == Some(AllocationResult::Existing) {
            Ok(AllocationResult::Created)
        } else {
            Err(AllocationError::VerificationFailed)
        }
    }

    fn verify_repository(&self, spec: &WorktreeSpec) -> Result<(), AllocationError> {
        let output = self.execute(spec, vec!["rev-parse".into(), "--show-toplevel".into()])?;
        let top = std::str::from_utf8(&output.stdout)
            .map_err(|error| AllocationError::MalformedOutput(error.to_string()))?
            .trim();
        let reported = Path::new(top)
            .canonicalize()
            .map_err(|error| AllocationError::Filesystem(error.to_string()))?;
        if reported != spec.repository() {
            return Err(AllocationError::VerificationFailed);
        }
        self.execute(
            spec,
            vec![
                "cat-file".into(),
                "-e".into(),
                format!("{}^{{commit}}", spec.base()),
            ],
        )?;
        Ok(())
    }

    fn worktrees(&self, spec: &WorktreeSpec) -> Result<Vec<WorktreeRecord>, AllocationError> {
        let output = self.execute(
            spec,
            vec![
                "worktree".into(),
                "list".into(),
                "--porcelain".into(),
                "-z".into(),
            ],
        )?;
        parse_worktrees(&output.stdout)
    }

    fn execute(
        &self,
        spec: &WorktreeSpec,
        args: Vec<String>,
    ) -> Result<GitOutput, AllocationError> {
        let output = self.raw(spec, args)?;
        if output.status != 0 {
            return Err(AllocationError::CommandFailed(output.status));
        }
        Ok(output)
    }

    fn raw(&self, spec: &WorktreeSpec, args: Vec<String>) -> Result<GitOutput, AllocationError> {
        let output = self.runner.run(&GitCommand {
            program: self.program.clone(),
            cwd: spec.repository().to_owned(),
            args,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
            environment: BTreeMap::new(),
        })?;
        if output.timed_out {
            return Err(AllocationError::TimedOut);
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(AllocationError::OutputTooLarge);
        }
        Ok(output)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorktreeRecord {
    path: PathBuf,
    branch: Option<String>,
}

fn parse_worktrees(payload: &[u8]) -> Result<Vec<WorktreeRecord>, AllocationError> {
    let mut records = Vec::new();
    let mut path = None;
    let mut branch = None;
    for raw in payload.split(|byte| *byte == 0) {
        if raw.is_empty() {
            if let Some(path) = path.take() {
                records.push(WorktreeRecord {
                    path,
                    branch: branch.take(),
                });
            } else if branch.is_some() {
                return Err(AllocationError::MalformedOutput(
                    "branch appeared without worktree path".into(),
                ));
            }
            continue;
        }
        let field = std::str::from_utf8(raw)
            .map_err(|error| AllocationError::MalformedOutput(error.to_string()))?;
        if let Some(value) = field.strip_prefix("worktree ") {
            if path.replace(PathBuf::from(value)).is_some() {
                return Err(AllocationError::MalformedOutput(
                    "record contains multiple worktree paths".into(),
                ));
            }
        } else if let Some(value) = field.strip_prefix("branch refs/heads/")
            && branch.replace(value.to_owned()).is_some()
        {
            return Err(AllocationError::MalformedOutput(
                "record contains multiple branches".into(),
            ));
        }
    }
    if let Some(path) = path {
        records.push(WorktreeRecord { path, branch });
    } else if branch.is_some() {
        return Err(AllocationError::MalformedOutput(
            "branch appeared without worktree path".into(),
        ));
    }
    Ok(records)
}

fn classify_existing(
    spec: &WorktreeSpec,
    records: &[WorktreeRecord],
) -> Result<Option<AllocationResult>, AllocationError> {
    classify_path_and_branch(spec.path(), spec.branch(), records)
}

fn classify_path_and_branch(
    path: &Path,
    branch: &str,
    records: &[WorktreeRecord],
) -> Result<Option<AllocationResult>, AllocationError> {
    let by_path = records.iter().find(|record| record.path == path);
    let by_branch = records
        .iter()
        .find(|record| record.branch.as_deref() == Some(branch));
    match (by_path, by_branch) {
        (None, None) => Ok(None),
        (Some(path), Some(branch)) if std::ptr::eq(path, branch) => {
            Ok(Some(AllocationResult::Existing))
        }
        _ => Err(AllocationError::Collision),
    }
}

fn classify_retirement_existing(
    spec: &WorktreeRetirementSpec,
    records: &[WorktreeRecord],
) -> Result<Option<AllocationResult>, AllocationError> {
    classify_path_and_branch(spec.path(), spec.branch(), records)
}
