//! Controller-only Hermes task projection writes.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{CommandRunner, CommandSpec, HermesError, TaskSnapshot, valid_id};

const CONTROLLER_IDENTITY: &str = "pip-controller";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TaskCreateSpec {
    pub board: String,
    pub effect_id: String,
    pub projection_key: String,
    pub title: String,
    pub body: Value,
    pub assignee: String,
    pub workspace: String,
    pub skills: Vec<String>,
    pub provider: String,
    pub model: String,
    pub max_runtime: String,
    pub max_retries: u32,
    pub priority: u32,
    pub parent_task_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionResult {
    Created(String),
    Existing(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    InvalidConfiguration,
    InvalidSpec,
    DuplicateProjection,
    ProjectionDrift,
    ArchivedProjection,
    Hermes(HermesError),
    MalformedResult(String),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid Hermes projector configuration")
            }
            Self::InvalidSpec => formatter.write_str("invalid Hermes task projection spec"),
            Self::DuplicateProjection => {
                formatter.write_str("multiple Hermes tasks have the same projection key")
            }
            Self::ProjectionDrift => {
                formatter.write_str("existing Hermes task differs from its projection")
            }
            Self::ArchivedProjection => {
                formatter.write_str("Hermes task was archived; explicit recovery is required")
            }
            Self::Hermes(error) => write!(formatter, "Hermes projection failed: {error}"),
            Self::MalformedResult(error) => {
                write!(
                    formatter,
                    "Hermes returned an invalid projected task: {error}"
                )
            }
        }
    }
}

impl std::error::Error for ProjectionError {}

impl From<HermesError> for ProjectionError {
    fn from(error: HermesError) -> Self {
        Self::Hermes(error)
    }
}

pub struct HermesProjector<R> {
    runner: R,
    program: String,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: CommandRunner> HermesProjector<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, ProjectionError> {
        let program = program.into();
        if program.trim().is_empty() || timeout.is_zero() || max_output_bytes == 0 {
            return Err(ProjectionError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            timeout,
            max_output_bytes,
        })
    }

    /// The caller must hold a fresh durable create reservation. A caller
    /// recovering an uncertain attempt must use `reconcile` instead.
    pub fn project(
        &self,
        spec: &TaskCreateSpec,
        observed: &[TaskSnapshot],
    ) -> Result<ProjectionResult, ProjectionError> {
        match self.reconcile(spec, observed)? {
            Some(id) => Ok(ProjectionResult::Existing(id)),
            None => self.create(spec),
        }
    }

    /// Read-only identity reconciliation. In particular, missing and archived
    /// tasks are not permission to retry an uncertain external create.
    pub fn reconcile(
        &self,
        spec: &TaskCreateSpec,
        observed: &[TaskSnapshot],
    ) -> Result<Option<String>, ProjectionError> {
        validate_spec(spec)?;

        let matches = observed
            .iter()
            .filter(|task| task_projection_key(task).as_deref() == Some(&spec.projection_key))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [task] if task.status == "archived" => Err(ProjectionError::ArchivedProjection),
            [task] if projection_matches(spec, task) => Ok(Some(task.id.clone())),
            [_] => Err(ProjectionError::ProjectionDrift),
            _ => Err(ProjectionError::DuplicateProjection),
        }
    }

    fn create(&self, spec: &TaskCreateSpec) -> Result<ProjectionResult, ProjectionError> {
        let max_runtime = hermes_runtime(&spec.max_runtime).ok_or(ProjectionError::InvalidSpec)?;
        let mut body = spec
            .body
            .as_object()
            .cloned()
            .ok_or(ProjectionError::InvalidSpec)?;
        match body.get("projection_key") {
            Some(Value::String(key)) if key == &spec.projection_key => {}
            Some(_) => return Err(ProjectionError::InvalidSpec),
            None => {
                body.insert(
                    "projection_key".into(),
                    Value::String(spec.projection_key.clone()),
                );
            }
        }
        let body = serde_json::to_string(&Value::Object(body))
            .map_err(|error| ProjectionError::MalformedResult(error.to_string()))?;

        let mut args = vec![
            "kanban".into(),
            "--board".into(),
            spec.board.clone(),
            "create".into(),
            spec.title.clone(),
            "--body".into(),
            body,
            "--assignee".into(),
            spec.assignee.clone(),
            "--workspace".into(),
            spec.workspace.clone(),
            "--idempotency-key".into(),
            spec.effect_id.clone(),
            "--created-by".into(),
            CONTROLLER_IDENTITY.into(),
            "--max-runtime".into(),
            max_runtime,
            "--max-retries".into(),
            spec.max_retries.to_string(),
            "--priority".into(),
            spec.priority.to_string(),
        ];
        for skill in &spec.skills {
            args.extend(["--skill".into(), skill.clone()]);
        }
        for parent in &spec.parent_task_ids {
            args.extend(["--parent".into(), parent.clone()]);
        }
        args.extend([
            "--model".into(),
            spec.model.clone(),
            "--provider".into(),
            spec.provider.clone(),
            "--json".into(),
        ]);

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
        let task: TaskSnapshot = serde_json::from_slice(&output.stdout)
            .map_err(|error| ProjectionError::MalformedResult(error.to_string()))?;
        if !valid_id(&task.id) || !projection_matches(spec, &task) {
            return Err(ProjectionError::MalformedResult(
                "created task does not match the requested projection".into(),
            ));
        }
        Ok(ProjectionResult::Created(task.id))
    }
}

fn validate_spec(spec: &TaskCreateSpec) -> Result<(), ProjectionError> {
    let valid = valid_id(&spec.board)
        && valid_opaque(&spec.effect_id, 512)
        && valid_opaque(&spec.projection_key, 512)
        && valid_text(&spec.title, 512)
        && spec.body.is_object()
        && valid_id(&spec.assignee)
        && valid_workspace(&spec.workspace)
        && !spec.skills.is_empty()
        && spec.skills.iter().all(|skill| valid_id(skill))
        && valid_id(&spec.provider)
        && spec.provider != "cursor"
        && valid_id(&spec.model)
        && hermes_runtime(&spec.max_runtime).is_some()
        && spec.max_retries > 0
        && spec.parent_task_ids.is_empty();
    if !valid {
        return Err(ProjectionError::InvalidSpec);
    }
    if let Some(existing) = spec.body.get("projection_key")
        && existing.as_str() != Some(spec.projection_key.as_str())
    {
        return Err(ProjectionError::InvalidSpec);
    }
    Ok(())
}

fn hermes_runtime(value: &str) -> Option<String> {
    let minutes = value
        .strip_prefix("PT")?
        .strip_suffix('M')?
        .parse::<u64>()
        .ok()?
        .checked_add(0)?;
    (1..=240).contains(&minutes).then(|| format!("{minutes}m"))
}

fn valid_workspace(value: &str) -> bool {
    if matches!(value, "scratch" | "worktree") {
        return true;
    }
    ["dir:", "worktree:"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .is_some_and(|path| path.starts_with('/') && valid_text(path, 4096))
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

fn projection_matches(spec: &TaskCreateSpec, task: &TaskSnapshot) -> bool {
    valid_id(&task.id)
        && task.title == spec.title
        && matches!(
            task.status.as_str(),
            "blocked" | "ready" | "running" | "in_progress" | "review" | "done"
        )
        && task.assignee.as_deref() == Some(spec.assignee.as_str())
        && task.created_by.as_deref() == Some(CONTROLLER_IDENTITY)
        && task_projection_key(task).as_deref() == Some(spec.projection_key.as_str())
        && serde_json::from_str::<Value>(&task.body).ok().as_ref()
            == Some(&{
                let mut body = spec.body.clone();
                body["projection_key"] = Value::String(spec.projection_key.clone());
                body
            })
        && {
            let (kind, path) = spec
                .workspace
                .split_once(':')
                .map_or((spec.workspace.as_str(), None), |(kind, path)| {
                    (kind, Some(path))
                });
            task.configuration.workspace_kind.as_deref() == Some(kind)
                && task.configuration.workspace_path.as_deref() == path
        }
        && task.configuration.skills.as_ref() == Some(&spec.skills)
        && task.configuration.provider_override.as_ref() == Some(&spec.provider)
        && task.configuration.model_override.as_ref() == Some(&spec.model)
        && task.configuration.max_retries == Some(spec.max_retries)
        && task.configuration.priority == Some(spec.priority)
}
