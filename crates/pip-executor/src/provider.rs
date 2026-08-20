//! Provider authentication, capability, and exact-model health probes.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::{ProcessError, ProcessOutput, ProcessRunner, ProcessSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthAssurance {
    AdvertisedExact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderHealth {
    pub provider: String,
    pub model: String,
    pub version: String,
    pub assurance: HealthAssurance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderProbeError {
    InvalidConfiguration,
    InvalidModel,
    Process(ProcessError),
    TimedOut,
    OutputTooLarge,
    CommandFailed(i32),
    InvalidUtf8,
    IncompatibleCli,
    AuthenticationUnverifiable,
    Unauthenticated,
    ModelNotAdvertised,
    AmbiguousModel,
}

impl fmt::Display for ProviderProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid provider probe configuration")
            }
            Self::InvalidModel => formatter.write_str("invalid exact provider model"),
            Self::Process(error) => error.fmt(formatter),
            Self::TimedOut => formatter.write_str("provider probe timed out"),
            Self::OutputTooLarge => formatter.write_str("provider probe output exceeded its bound"),
            Self::CommandFailed(status) => write!(formatter, "provider probe exited with {status}"),
            Self::InvalidUtf8 => formatter.write_str("provider probe output is not UTF-8"),
            Self::IncompatibleCli => {
                formatter.write_str("provider CLI lacks the required safe probe commands")
            }
            Self::AuthenticationUnverifiable => {
                formatter.write_str("provider authentication could not be verified")
            }
            Self::Unauthenticated => formatter.write_str("provider is not authenticated"),
            Self::ModelNotAdvertised => formatter.write_str("required model is not advertised"),
            Self::AmbiguousModel => {
                formatter.write_str("required model is advertised more than once")
            }
        }
    }
}

impl std::error::Error for ProviderProbeError {}

impl From<ProcessError> for ProviderProbeError {
    fn from(error: ProcessError) -> Self {
        Self::Process(error)
    }
}

pub struct CursorHealthProbe<R> {
    runner: R,
    program: String,
    cwd: PathBuf,
    environment: BTreeMap<String, String>,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: ProcessRunner> CursorHealthProbe<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        cwd: PathBuf,
        environment: BTreeMap<String, String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, ProviderProbeError> {
        let program = program.into();
        if program.trim().is_empty()
            || !cwd.is_absolute()
            || timeout.is_zero()
            || max_output_bytes == 0
        {
            return Err(ProviderProbeError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            cwd,
            environment,
            timeout,
            max_output_bytes,
        })
    }

    pub fn probe(&self, model: &str) -> Result<ProviderHealth, ProviderProbeError> {
        if !valid_model(model) {
            return Err(ProviderProbeError::InvalidModel);
        }
        let version = self.text(vec!["--version".into()])?;
        let version = version.trim();
        if version.is_empty() {
            return Err(ProviderProbeError::IncompatibleCli);
        }
        let help = self.text(vec!["--help".into()])?;
        let supports_models = help.lines().any(|line| {
            let line = line.trim();
            line == "models" || line.starts_with("models ")
        });
        if !supports_models {
            return Err(ProviderProbeError::IncompatibleCli);
        }
        let status = self.text(vec!["status".into()])?;
        let normalized = status.to_ascii_lowercase();
        if ["not authenticated", "not logged in", "unauthenticated"]
            .iter()
            .any(|marker| normalized.contains(marker))
        {
            return Err(ProviderProbeError::Unauthenticated);
        }
        if !normalized.contains("authenticated") && !normalized.contains("logged in") {
            return Err(ProviderProbeError::AuthenticationUnverifiable);
        }
        let models = self.text(vec!["models".into()])?;
        let exact_count = models
            .lines()
            .filter_map(model_token)
            .filter(|advertised| *advertised == model)
            .count();
        match exact_count {
            0 => Err(ProviderProbeError::ModelNotAdvertised),
            1 => Ok(ProviderHealth {
                provider: "cursor".into(),
                model: model.into(),
                version: version.into(),
                assurance: HealthAssurance::AdvertisedExact,
            }),
            _ => Err(ProviderProbeError::AmbiguousModel),
        }
    }

    fn text(&self, args: Vec<String>) -> Result<String, ProviderProbeError> {
        let output = self.execute(args)?;
        String::from_utf8(output.stdout).map_err(|_| ProviderProbeError::InvalidUtf8)
    }

    fn execute(&self, args: Vec<String>) -> Result<ProcessOutput, ProviderProbeError> {
        let output = self.runner.run(&ProcessSpec {
            program: self.program.clone(),
            args,
            cwd: self.cwd.clone(),
            environment: self.environment.clone(),
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        if output.timed_out {
            return Err(ProviderProbeError::TimedOut);
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(ProviderProbeError::OutputTooLarge);
        }
        if output.status != 0 {
            return Err(ProviderProbeError::CommandFailed(output.status));
        }
        Ok(output)
    }
}

fn valid_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn model_token(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let line = line
        .strip_prefix('-')
        .or_else(|| line.strip_prefix('*'))
        .unwrap_or(line)
        .trim();
    line.split_whitespace().next()
}
