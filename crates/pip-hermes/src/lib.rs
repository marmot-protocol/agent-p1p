//! Read-only Hermes capability and task adapter.

#![forbid(unsafe_code)]

mod bootstrap;
mod gate;
mod projection;

pub use bootstrap::{
    BootstrapError, BootstrapOutcome, HermesBootstrap, ProfileBootstrapSpec, RuntimeBootstrapSpec,
};

pub use gate::{
    GateCreateSpec, GateError, GateProjectionResult, GateReleaseResult, HermesGateController,
};

pub use projection::{HermesProjector, ProjectionError, ProjectionResult, TaskCreateSpec};

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wait_timeout::ChildExt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

pub trait CommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError>;
}

#[derive(Clone, Debug)]
pub struct ProcessRunner {
    environment: BTreeMap<OsString, OsString>,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        let mut environment = BTreeMap::new();
        environment.insert(
            OsString::from("PATH"),
            std::env::var_os("PATH")
                .unwrap_or_else(|| OsString::from("/usr/local/bin:/usr/bin:/bin")),
        );
        for name in [
            "HOME",
            "HERMES_HOME",
            "HERMES_KANBAN_HOME",
            "HERMES_KANBAN_BOARD",
        ] {
            if let Some(value) = std::env::var_os(name) {
                environment.insert(OsString::from(name), value);
            }
        }
        Self { environment }
    }
}

impl ProcessRunner {
    pub fn for_hermes_root(root: &Path) -> Result<Self, HermesError> {
        if !root.is_absolute() || root == Path::new("/") {
            return Err(HermesError::InvalidConfiguration);
        }
        let mut runner = Self::default();
        runner
            .environment
            .insert(OsString::from("HOME"), root.join("home").into_os_string());
        runner
            .environment
            .insert(OsString::from("HERMES_HOME"), root.as_os_str().to_owned());
        runner.environment.insert(
            OsString::from("HERMES_KANBAN_HOME"),
            root.as_os_str().to_owned(),
        );
        runner
            .environment
            .remove(&OsString::from("HERMES_KANBAN_BOARD"));
        Ok(runner)
    }
}

impl CommandRunner for ProcessRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, HermesError> {
        if spec.program.trim().is_empty() || spec.timeout.is_zero() || spec.max_output_bytes == 0 {
            return Err(HermesError::InvalidConfiguration);
        }
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| HermesError::Process(error.to_string()))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            HermesError::Process("failed to capture child standard output".into())
        })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HermesError::Process("failed to capture child standard error".into()))?;
        let limit = spec.max_output_bytes.saturating_add(1);
        let stdout_reader = thread::spawn(move || drain_bounded(stdout, limit));
        let stderr_reader = thread::spawn(move || drain_bounded(stderr, limit));

        let status = child
            .wait_timeout(spec.timeout)
            .map_err(|error| HermesError::Process(error.to_string()))?;
        let timed_out = status.is_none();
        let status = if let Some(status) = status {
            status
        } else {
            child
                .kill()
                .map_err(|error| HermesError::Process(error.to_string()))?;
            child
                .wait()
                .map_err(|error| HermesError::Process(error.to_string()))?
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| HermesError::Process("stdout reader panicked".into()))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| HermesError::Process("stderr reader panicked".into()))??;
        Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
            timed_out,
        })
    }
}

fn drain_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, HermesError> {
    let mut retained = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| HermesError::Process(error.to_string()))?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HermesError {
    InvalidConfiguration,
    InvalidBoard,
    InvalidTask,
    TimedOut,
    OutputTooLarge,
    CommandFailed(i32),
    InvalidUtf8,
    MalformedJson(String),
    IncompleteTask,
    IncompleteRun,
    RetryLimitReached,
    InvalidRunMetadata,
    Process(String),
}

impl fmt::Display for HermesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid Hermes reader configuration")
            }
            Self::InvalidBoard => formatter.write_str("invalid Hermes board name"),
            Self::InvalidTask => formatter.write_str("invalid Hermes task identity"),
            Self::TimedOut => formatter.write_str("Hermes command timed out"),
            Self::OutputTooLarge => {
                formatter.write_str("Hermes output exceeded the configured bound")
            }
            Self::CommandFailed(status) => write!(formatter, "Hermes command exited with {status}"),
            Self::InvalidUtf8 => formatter.write_str("Hermes output is not UTF-8"),
            Self::MalformedJson(error) => write!(formatter, "malformed Hermes JSON: {error}"),
            Self::IncompleteTask => formatter.write_str("Hermes task is not durably complete"),
            Self::IncompleteRun => formatter.write_str("Hermes task has no successful latest run"),
            Self::RetryLimitReached => {
                formatter.write_str("Hermes task reached its configured retry limit")
            }
            Self::InvalidRunMetadata => {
                formatter.write_str("Hermes run metadata is not a JSON object")
            }
            Self::Process(error) => write!(formatter, "Hermes process failed: {error}"),
        }
    }
}

impl std::error::Error for HermesError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub title: String,
    pub status: String,
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskRunSnapshot {
    pub outcome: Option<String>,
    pub profile: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskDetail {
    pub task: TaskSnapshot,
    #[serde(default)]
    pub runs: Vec<TaskRunSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompletedTaskResult {
    pub task: TaskSnapshot,
    pub profile: String,
    pub metadata: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub version: String,
    pub boards: Vec<String>,
}

#[derive(Deserialize)]
struct BoardSnapshot {
    slug: String,
}

pub struct HermesReader<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: CommandRunner> HermesReader<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, HermesError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(HermesError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            timeout,
            max_output_bytes,
        })
    }

    pub fn capabilities(&self) -> Result<Capabilities, HermesError> {
        let version_output = self.execute(vec!["--version".into()])?;
        let version = std::str::from_utf8(&version_output.stdout)
            .map_err(|_| HermesError::InvalidUtf8)?
            .trim()
            .to_owned();
        if version.is_empty() {
            return Err(HermesError::InvalidUtf8);
        }
        let boards: Vec<BoardSnapshot> = self.execute_json(vec![
            "kanban".into(),
            "boards".into(),
            "list".into(),
            "--json".into(),
        ])?;
        let mut names = Vec::with_capacity(boards.len());
        for board in boards {
            if !valid_id(&board.slug) {
                return Err(HermesError::InvalidBoard);
            }
            names.push(board.slug);
        }
        Ok(Capabilities {
            version,
            boards: names,
        })
    }

    pub fn list_tasks(&self, board: &str) -> Result<Vec<TaskSnapshot>, HermesError> {
        if !valid_id(board) {
            return Err(HermesError::InvalidBoard);
        }
        self.execute_json(vec![
            "kanban".into(),
            "--board".into(),
            board.into(),
            "list".into(),
            "--archived".into(),
            "--json".into(),
        ])
    }

    pub fn show_task(&self, board: &str, task_id: &str) -> Result<TaskSnapshot, HermesError> {
        if !valid_id(board) {
            return Err(HermesError::InvalidBoard);
        }
        if !valid_id(task_id) {
            return Err(HermesError::InvalidTask);
        }
        self.execute_json(vec![
            "kanban".into(),
            "--board".into(),
            board.into(),
            "show".into(),
            task_id.into(),
            "--json".into(),
        ])
    }

    pub fn show_completed_result(
        &self,
        board: &str,
        task_id: &str,
    ) -> Result<CompletedTaskResult, HermesError> {
        if !valid_id(board) {
            return Err(HermesError::InvalidBoard);
        }
        if !valid_id(task_id) {
            return Err(HermesError::InvalidTask);
        }
        let detail: TaskDetail = self.execute_json(vec![
            "kanban".into(),
            "--board".into(),
            board.into(),
            "show".into(),
            task_id.into(),
            "--json".into(),
        ])?;
        if detail.task.id != task_id {
            return Err(HermesError::IncompleteTask);
        }
        if detail.task.status == "blocked"
            && detail.runs.last().and_then(|run| run.outcome.as_deref()) == Some("gave_up")
        {
            return Err(HermesError::RetryLimitReached);
        }
        if detail.task.status != "done" {
            return Err(HermesError::IncompleteTask);
        }
        let run = detail.runs.last().ok_or(HermesError::IncompleteRun)?;
        if run.outcome.as_deref() != Some("completed") {
            return Err(HermesError::IncompleteRun);
        }
        let profile = run
            .profile
            .as_deref()
            .filter(|profile| valid_id(profile))
            .ok_or(HermesError::IncompleteRun)?;
        let metadata = match run.metadata.as_ref() {
            Some(Value::Object(_)) => run.metadata.clone().expect("matched metadata"),
            Some(Value::String(encoded)) => serde_json::from_str(encoded)
                .map_err(|error| HermesError::MalformedJson(error.to_string()))?,
            _ => return Err(HermesError::InvalidRunMetadata),
        };
        if !metadata.is_object() {
            return Err(HermesError::InvalidRunMetadata);
        }
        Ok(CompletedTaskResult {
            task: detail.task,
            profile: profile.into(),
            metadata,
        })
    }

    fn execute_json<T: for<'de> Deserialize<'de>>(
        &self,
        args: Vec<String>,
    ) -> Result<T, HermesError> {
        let output = self.execute(args)?;
        serde_json::from_slice(&output.stdout)
            .map_err(|error| HermesError::MalformedJson(error.to_string()))
    }

    fn execute(&self, args: Vec<String>) -> Result<CommandOutput, HermesError> {
        let output = self.runner.run(&CommandSpec {
            program: self.program.clone(),
            args,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        if output.timed_out {
            return Err(HermesError::TimedOut);
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(HermesError::OutputTooLarge);
        }
        if output.status != 0 {
            return Err(HermesError::CommandFailed(output.status));
        }
        Ok(output)
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesiredTask {
    pub projection_key: String,
    pub title: String,
    pub assignee: String,
    pub desired_status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Discrepancy {
    Drifted(String),
    Missing(String),
    Duplicate(String),
    Foreign(String),
}

#[must_use]
pub fn compare_projection(desired: &[DesiredTask], observed: &[TaskSnapshot]) -> Vec<Discrepancy> {
    let mut by_key: BTreeMap<String, Vec<&TaskSnapshot>> = BTreeMap::new();
    let mut foreign = Vec::new();
    for task in observed {
        let key = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&task.body)
            .ok()
            .and_then(|body| {
                body.get("projection_key")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            });
        if let Some(key) = key {
            by_key.entry(key).or_default().push(task);
        } else {
            foreign.push(task.id.clone());
        }
    }

    let desired_keys: BTreeSet<_> = desired
        .iter()
        .map(|task| task.projection_key.as_str())
        .collect();
    let mut discrepancies = Vec::new();
    for task in desired {
        match by_key.get(&task.projection_key).map(Vec::as_slice) {
            None | Some([]) => {
                discrepancies.push(Discrepancy::Missing(task.projection_key.clone()))
            }
            Some([observed]) => {
                if observed.title != task.title
                    || observed.assignee.as_deref() != Some(task.assignee.as_str())
                    || observed.status != task.desired_status
                {
                    discrepancies.push(Discrepancy::Drifted(task.projection_key.clone()));
                }
            }
            Some(_) => discrepancies.push(Discrepancy::Duplicate(task.projection_key.clone())),
        }
    }
    for (key, tasks) in by_key {
        if !desired_keys.contains(key.as_str()) {
            foreign.extend(tasks.into_iter().map(|task| task.id.clone()));
        }
    }
    foreign.sort();
    discrepancies.extend(foreign.into_iter().map(Discrepancy::Foreign));
    discrepancies
}
