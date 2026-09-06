//! Bounded, shell-free subprocess execution.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};
use wait_timeout::ChildExt;

#[derive(Clone, Eq, PartialEq)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    /// An existing input artifact, or closed stdin when absent.
    pub stdin_file: Option<PathBuf>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessOutput {
    /// Normal exit code, or negative Unix termination signal.
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessError {
    InvalidSpec,
    Spawn(String),
    Io(String),
    ReaderPanicked,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec => formatter.write_str("invalid bounded process spec"),
            Self::Spawn(error) => write!(formatter, "failed to spawn bounded process: {error}"),
            Self::Io(error) => write!(formatter, "bounded process I/O failed: {error}"),
            Self::ReaderPanicked => formatter.write_str("bounded process output reader panicked"),
        }
    }
}

impl std::error::Error for ProcessError {}

pub trait ProcessRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BoundedProcessRunner;

impl ProcessRunner for BoundedProcessRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput, ProcessError> {
        if spec.program.trim().is_empty()
            || !spec.cwd.is_absolute()
            || !spec.cwd.is_dir()
            || spec.timeout.is_zero()
            || spec.max_output_bytes == 0
            || spec
                .environment
                .keys()
                .any(|key| key.is_empty() || key.contains('=') || key.contains('\0'))
            || spec.environment.values().any(|value| value.contains('\0'))
        {
            return Err(ProcessError::InvalidSpec);
        }
        let stdin = match &spec.stdin_file {
            Some(path) => {
                if !path.is_absolute() {
                    return Err(ProcessError::InvalidSpec);
                }
                let input =
                    File::open(path).map_err(|error| ProcessError::Io(error.to_string()))?;
                if !input
                    .metadata()
                    .map_err(|error| ProcessError::Io(error.to_string()))?
                    .is_file()
                {
                    return Err(ProcessError::InvalidSpec);
                }
                Stdio::from(input)
            }
            None => Stdio::null(),
        };
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.environment)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| ProcessError::Spawn(error.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io("failed to capture child standard output".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProcessError::Io("failed to capture child standard error".into()))?;
        let limit = spec.max_output_bytes.saturating_add(1);
        let stdout_reader = thread::spawn(move || drain_bounded(stdout, limit));
        let stderr_reader = thread::spawn(move || drain_bounded(stderr, limit));
        let status = child
            .wait_timeout(spec.timeout)
            .map_err(|error| ProcessError::Io(error.to_string()))?;
        let timed_out = status.is_none();
        let status = if let Some(status) = status {
            status
        } else {
            terminate_process_group(child.id());
            let _ = child.kill();
            child
                .wait()
                .map_err(|error| ProcessError::Io(error.to_string()))?
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| ProcessError::ReaderPanicked)??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| ProcessError::ReaderPanicked)??;
        Ok(ProcessOutput {
            status: status.code().unwrap_or_else(|| {
                #[cfg(unix)]
                {
                    -status.signal().unwrap_or(1)
                }
                #[cfg(not(unix))]
                {
                    -1
                }
            }),
            stdout,
            stderr,
            timed_out,
        })
    }
}

fn drain_bounded(mut reader: impl Read, limit: usize) -> Result<Vec<u8>, ProcessError> {
    let mut retained = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ProcessError::Io(error.to_string()))?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn terminate_process_group(_pid: u32) {}

#[must_use]
pub fn sanitized_environment() -> BTreeMap<String, String> {
    const ALLOWED: &[&str] = &[
        "HOME",
        "LANG",
        "LC_ALL",
        "NO_PROXY",
        "PATH",
        "SSL_CERT_DIR",
        "SSL_CERT_FILE",
        "TERM",
        "TMPDIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ];
    std::env::vars()
        .filter(|(key, _)| ALLOWED.contains(&key.as_str()))
        .collect()
}
