//! Controller-only activation-gate creation and release.

use std::fmt;
use std::time::Duration;

use serde_json::{Map, Value};

use super::{CommandOutput, CommandRunner, CommandSpec, HermesError, TaskSnapshot, valid_id};

const CONTROLLER_IDENTITY: &str = "pip-controller";

#[derive(Clone, Debug, PartialEq)]
pub struct GateCreateSpec {
    pub board: String,
    pub effect_id: String,
    pub projection_key: String,
    pub title: String,
    pub body: Value,
    pub parent_task_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateProjectionResult {
    Created(String),
    Existing(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateReleaseResult {
    Released,
    AlreadyReleased,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GateError {
    InvalidConfiguration,
    InvalidSpec,
    DuplicateProjection,
    ProjectionDrift,
    UnsafeGateState,
    Hermes(HermesError),
    MalformedResult(String),
}

impl fmt::Display for GateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => formatter.write_str("invalid Hermes gate configuration"),
            Self::InvalidSpec => formatter.write_str("invalid Hermes activation gate spec"),
            Self::DuplicateProjection => {
                formatter.write_str("multiple Hermes activation gates have the same key")
            }
            Self::ProjectionDrift => {
                formatter.write_str("Hermes activation gate differs from its projection")
            }
            Self::UnsafeGateState => {
                formatter.write_str("Hermes activation gate is not safely releasable")
            }
            Self::Hermes(error) => error.fmt(formatter),
            Self::MalformedResult(error) => {
                write!(formatter, "invalid Hermes gate result: {error}")
            }
        }
    }
}

impl std::error::Error for GateError {}

impl From<HermesError> for GateError {
    fn from(error: HermesError) -> Self {
        Self::Hermes(error)
    }
}

pub struct HermesGateController<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: CommandRunner> HermesGateController<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, GateError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(GateError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            timeout,
            max_output_bytes,
        })
    }

    pub fn project(
        &self,
        spec: &GateCreateSpec,
        observed: &[TaskSnapshot],
    ) -> Result<GateProjectionResult, GateError> {
        validate_spec(spec)?;
        let matches = observed
            .iter()
            .filter(|task| task_projection_key(task).as_deref() == Some(&spec.projection_key))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => self.create(spec),
            [task] if gate_matches(spec, task) => {
                Ok(GateProjectionResult::Existing(task.id.clone()))
            }
            [_] => Err(GateError::ProjectionDrift),
            _ => Err(GateError::DuplicateProjection),
        }
    }

    pub fn release(
        &self,
        spec: &GateCreateSpec,
        observed: &TaskSnapshot,
        reason: &str,
    ) -> Result<GateReleaseResult, GateError> {
        validate_spec(spec)?;
        if !valid_text(reason, 1024) || !gate_matches(spec, observed) {
            return Err(GateError::UnsafeGateState);
        }
        match observed.status.as_str() {
            "done" => Ok(GateReleaseResult::AlreadyReleased),
            "blocked" => {
                self.execute(vec![
                    "kanban".into(),
                    "--board".into(),
                    spec.board.clone(),
                    "complete".into(),
                    observed.id.clone(),
                    "--result".into(),
                    reason.into(),
                ])?;
                Ok(GateReleaseResult::Released)
            }
            _ => Err(GateError::UnsafeGateState),
        }
    }

    fn create(&self, spec: &GateCreateSpec) -> Result<GateProjectionResult, GateError> {
        let mut body = spec
            .body
            .as_object()
            .cloned()
            .ok_or(GateError::InvalidSpec)?;
        body.insert(
            "projection_key".into(),
            Value::String(spec.projection_key.clone()),
        );
        let body = serde_json::to_string(&Value::Object(body))
            .map_err(|error| GateError::MalformedResult(error.to_string()))?;
        let mut args = vec![
            "kanban".into(),
            "--board".into(),
            spec.board.clone(),
            "create".into(),
            spec.title.clone(),
            "--body".into(),
            body,
            "--idempotency-key".into(),
            spec.effect_id.clone(),
            "--created-by".into(),
            CONTROLLER_IDENTITY.into(),
            "--max-retries".into(),
            "1".into(),
        ];
        for parent in &spec.parent_task_ids {
            args.extend(["--parent".into(), parent.clone()]);
        }
        args.extend(["--initial-status".into(), "blocked".into(), "--json".into()]);
        let output = self.execute(args)?;
        let task: TaskSnapshot = serde_json::from_slice(&output.stdout)
            .map_err(|error| GateError::MalformedResult(error.to_string()))?;
        if !gate_matches(spec, &task) || task.status != "blocked" {
            return Err(GateError::MalformedResult(
                "created task does not match the activation gate".into(),
            ));
        }
        Ok(GateProjectionResult::Created(task.id))
    }

    fn execute(&self, args: Vec<String>) -> Result<CommandOutput, GateError> {
        let output = self.runner.run(&CommandSpec {
            program: self.program.clone(),
            args,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        if output.timed_out {
            return Err(HermesError::TimedOut.into());
        }
        if output.stdout.len() > self.max_output_bytes
            || output.stderr.len() > self.max_output_bytes
        {
            return Err(HermesError::OutputTooLarge.into());
        }
        if output.status != 0 {
            return Err(HermesError::CommandFailed(output.status).into());
        }
        Ok(output)
    }
}

fn validate_spec(spec: &GateCreateSpec) -> Result<(), GateError> {
    let valid = valid_id(&spec.board)
        && valid_opaque(&spec.effect_id, 512)
        && valid_opaque(&spec.projection_key, 512)
        && valid_text(&spec.title, 512)
        && spec.body.is_object()
        && spec.parent_task_ids.iter().all(|parent| valid_id(parent));
    if !valid
        || spec
            .body
            .get("projection_key")
            .is_some_and(|value| value.as_str() != Some(spec.projection_key.as_str()))
    {
        return Err(GateError::InvalidSpec);
    }
    Ok(())
}

fn valid_opaque(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace())
}

fn valid_text(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|character| !character.is_control() || character == '\n' || character == '\t')
}

fn task_projection_key(task: &TaskSnapshot) -> Option<String> {
    serde_json::from_str::<Map<String, Value>>(&task.body)
        .ok()?
        .get("projection_key")?
        .as_str()
        .map(str::to_owned)
}

fn gate_matches(spec: &GateCreateSpec, task: &TaskSnapshot) -> bool {
    valid_id(&task.id)
        && task.title == spec.title
        && matches!(task.status.as_str(), "blocked" | "done")
        && task.assignee.is_none()
        && task.created_by.as_deref() == Some(CONTROLLER_IDENTITY)
        && task_projection_key(task).as_deref() == Some(spec.projection_key.as_str())
}
