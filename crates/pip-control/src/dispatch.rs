//! Ledger-first, gate-free Hermes dispatch with fail-closed create recovery.

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

use pip_controller::{DirectTaskSpec, DispatchError, ExecutionKind, schedule_claimed_dispatch};
use pip_core::GitSha;
use pip_hermes::{
    CommandRunner, HermesError, HermesProjector, HermesReader, ProcessRunner, ProjectionError,
    ProjectionResult, TaskCreateSpec, TaskSnapshot,
};
use pip_store::{
    ApplyResult, CreateReservation, DispatchIntent, DispatchTransport, EffectInput, Store,
    StoreError, TaskProjectionInput,
};
use serde::Serialize;

use crate::{GitWorkspacePreparer, PolicyError, WorkspaceError, WorkspacePreparer};

const DISPATCH_EFFECTS: [&str; 4] = [
    "DISPATCH_PLANNER",
    "DISPATCH_BUILDER",
    "DISPATCH_REVIEWERS",
    "DISPATCH_FINAL_REVIEWER",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DispatchCycleResult {
    Idle,
    AuthorizationBlocked,
    Projected {
        effect_id: String,
        projection_count: usize,
        direct_job_count: usize,
        released_gate_count: usize,
        ledger_result: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchCycleContext<'a> {
    pub skills_repository_commit: &'a str,
    pub hermes_program: &'a str,
    pub owner: &'a str,
    pub now: u64,
    pub lease_seconds: u64,
    pub authorization_valid: bool,
}

#[derive(Debug)]
pub enum DispatchCycleError {
    Store(StoreError),
    Policy(PolicyError),
    Schedule(DispatchError),
    Hermes(HermesError),
    UncertainCreate(String),
    Projection(ProjectionError),
    Workspace(WorkspaceError),
    Serialization(String),
    DispatchPaused,
    InvalidSkillsCommit,
}

impl fmt::Display for DispatchCycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Policy(error) => error.fmt(formatter),
            Self::Schedule(error) => error.fmt(formatter),
            Self::Hermes(error) => error.fmt(formatter),
            Self::UncertainCreate(id) => write!(
                formatter,
                "Hermes create outcome is uncertain for {id}; reconcile before retrying"
            ),
            Self::Projection(error) => error.fmt(formatter),
            Self::Workspace(error) => error.fmt(formatter),
            Self::Serialization(error) => {
                write!(formatter, "dispatch serialization failed: {error}")
            }
            Self::DispatchPaused => {
                formatter.write_str("repository dispatch is disabled or paused")
            }
            Self::InvalidSkillsCommit => {
                formatter.write_str("dispatch requires an exact skills repository commit")
            }
        }
    }
}

impl std::error::Error for DispatchCycleError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for DispatchCycleError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

impl From<serde_json::Error> for DispatchCycleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}

error_from!(StoreError, Store);
error_from!(PolicyError, Policy);
error_from!(DispatchError, Schedule);
error_from!(HermesError, Hermes);
error_from!(ProjectionError, Projection);
error_from!(WorkspaceError, Workspace);

pub fn dispatch_once<'a>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    context: DispatchCycleContext<'_>,
) -> Result<DispatchCycleResult, DispatchCycleError> {
    dispatch_once_with_workspace(
        store,
        scope,
        ProcessRunner::default(),
        &GitWorkspacePreparer,
        context,
    )
}

pub fn dispatch_once_with<'a, R: CommandRunner + Clone>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    runner: R,
    context: DispatchCycleContext<'_>,
) -> Result<DispatchCycleResult, DispatchCycleError> {
    dispatch_once_inner(store, scope, runner, None, context)
}

pub fn dispatch_once_with_workspace<'a, R: CommandRunner + Clone, W: WorkspacePreparer>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    runner: R,
    workspace: &W,
    context: DispatchCycleContext<'_>,
) -> Result<DispatchCycleResult, DispatchCycleError> {
    dispatch_once_inner(store, scope, runner, Some(workspace), context)
}

fn dispatch_once_inner<'a, R: CommandRunner + Clone>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    runner: R,
    workspace: Option<&dyn WorkspacePreparer>,
    context: DispatchCycleContext<'_>,
) -> Result<DispatchCycleResult, DispatchCycleError> {
    let scope = scope.into();
    let policy = scope.policy;
    let started = Instant::now();
    // Include time spent preparing workspaces and waiting on external commands
    // when checking a lease; a cycle's initial timestamp is not a frozen clock.
    let now = || context.now.saturating_add(started.elapsed().as_secs());
    if !policy.dispatch_enabled || policy.intake.paused {
        return Err(DispatchCycleError::DispatchPaused);
    }
    if !context.authorization_valid {
        return Ok(DispatchCycleResult::AuthorizationBlocked);
    }
    let skills_repository_commit = GitSha::from_str(context.skills_repository_commit)
        .map_err(|_| DispatchCycleError::InvalidSkillsCommit)?;
    let Some(claimed) = scope.claim(
        store,
        context.owner,
        context.now,
        context.lease_seconds,
        &DISPATCH_EFFECTS,
    )?
    else {
        return Ok(DispatchCycleResult::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or_else(|| StoreError::MissingCase(claimed.case_key.clone()))?;
    if let Some(workspace) = workspace {
        workspace.prepare(policy, &claimed, &case, store)?;
    }
    // Existing work is an immutable saved job, not a request to render today's
    // profiles/skills/history again. Claim validation still fences stale work.
    let intents = if let Some(saved) = store.dispatch_intents(&claimed.effect_id)? {
        saved
    } else {
        let dispatches = schedule_claimed_dispatch(
            &claimed,
            &case,
            store,
            &policy.workflow_policy()?,
            skills_repository_commit,
        )?;
        let intents = dispatches
            .iter()
            .map(|dispatch| {
                let (transport, desired) = match dispatch.execution() {
                    ExecutionKind::Hermes => (
                        DispatchTransport::Hermes,
                        serde_json::to_value(dispatch.hermes_task()?)?,
                    ),
                    ExecutionKind::Direct => (
                        DispatchTransport::Direct,
                        serde_json::to_value(dispatch.direct_task()?)?,
                    ),
                };
                Ok(DispatchIntent {
                    intent_id: dispatch.worker_projection_key.clone(),
                    transport,
                    desired,
                })
            })
            .collect::<Result<Vec<_>, DispatchCycleError>>()?;
        // Freeze ALL roles atomically before the first queue write. A deployment or
        // policy change cannot silently replace a previously authorized model/body.
        store.freeze_dispatch_intents(&claimed, &intents, now())?;
        intents
    };
    if let Some(workspace) = workspace {
        workspace.prepare_dispatch_storage(policy, store, &intents)?;
    }
    let has_hermes = intents
        .iter()
        .any(|intent| intent.transport == DispatchTransport::Hermes);
    let timeout = Duration::from_secs(20);
    let output_bound = 4 * 1024 * 1024;
    let reader = has_hermes
        .then(|| {
            HermesReader::new(
                runner.clone(),
                context.hermes_program,
                timeout,
                output_bound,
            )
        })
        .transpose()?;
    let projector = has_hermes
        .then(|| HermesProjector::new(runner, context.hermes_program, timeout, output_bound))
        .transpose()?;
    let mut projections = Vec::with_capacity(intents.len());
    let mut direct_jobs = Vec::with_capacity(intents.len());
    for intent in intents {
        if intent.transport == DispatchTransport::Direct {
            let task: DirectTaskSpec = serde_json::from_value(intent.desired)?;
            let role = task
                .body
                .get("role")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    DispatchCycleError::Serialization("direct task role is missing".into())
                })?;
            let worker_id = task
                .body
                .get("reviewer_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(role);
            let observer = task
                .body
                .get("review_mode")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|mode| matches!(mode, "advisory" | "shadow"));
            direct_jobs.push(EffectInput {
                effect_id: format!("{}:direct:{worker_id}", claimed.effect_id),
                effect_type: if observer {
                    "RUN_DIRECT_OBSERVER"
                } else {
                    "RUN_DIRECT_WORKER"
                }
                .into(),
                payload: serde_json::to_value(&task)?,
            });
            continue;
        }
        let worker_spec: TaskCreateSpec = serde_json::from_value(intent.desired)?;
        let reader = reader
            .as_ref()
            .expect("Hermes reader exists for Hermes dispatch");
        let projector = projector
            .as_ref()
            .expect("Hermes projector exists for Hermes dispatch");
        // Reservation is durable BEFORE any create. It is never recycled:
        // another controller may reconcile but cannot compete with a still-live
        // subprocess, even if the originating controller died or its lease expired.
        let observed = reader.list_tasks(&policy.board)?;
        let existing = projector.reconcile(&worker_spec, &observed)?;
        let reservation = store.reserve_dispatch_create(&claimed, &intent.intent_id, now())?;
        let worker_id = match existing {
            Some(id) => id,
            None if reservation == CreateReservation::Granted => {
                match projector.project(&worker_spec, &observed)? {
                    ProjectionResult::Created(id) | ProjectionResult::Existing(id) => id,
                }
            }
            None => return Err(DispatchCycleError::UncertainCreate(intent.intent_id)),
        };
        // Hermes show returns an envelope, not a bare task. Verify identity and
        // dependencies again: the task can already be running or done here.
        let detail = reader.show_task_detail(&policy.board, &worker_id)?;
        if detail.parents.as_deref() != Some(&[])
            || projector
                .reconcile(&worker_spec, std::slice::from_ref(&detail.task))?
                .as_deref()
                != Some(worker_id.as_str())
        {
            return Err(ProjectionError::ProjectionDrift.into());
        }
        projections.push(projection(
            &worker_spec.projection_key,
            &claimed.effect_id,
            &policy.board,
            &detail.task,
            &worker_spec,
        )?);
    }
    let ledger = store.complete_dispatch_outputs(
        &claimed.effect_id,
        &projections,
        &direct_jobs,
        context.owner,
        now(),
        None,
    )?;
    Ok(DispatchCycleResult::Projected {
        effect_id: claimed.effect_id,
        projection_count: projections.len(),
        direct_job_count: direct_jobs.len(),
        released_gate_count: 0,
        ledger_result: match ledger {
            ApplyResult::Applied => "applied",
            ApplyResult::Replayed => "replayed",
        }
        .into(),
    })
}

fn projection(
    projection_id: &str,
    effect_id: &str,
    board: &str,
    observed: &TaskSnapshot,
    desired: &impl Serialize,
) -> Result<TaskProjectionInput, DispatchCycleError> {
    Ok(TaskProjectionInput {
        projection_id: projection_id.into(),
        effect_id: effect_id.into(),
        board: board.into(),
        task_id: observed.id.clone(),
        desired: serde_json::to_value(desired)
            .map_err(|error| DispatchCycleError::Serialization(error.to_string()))?,
        observed: serde_json::to_value(observed)
            .map_err(|error| DispatchCycleError::Serialization(error.to_string()))?,
    })
}
