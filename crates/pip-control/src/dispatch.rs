//! Restart-safe durable outbox projection into controller-owned Hermes gates.

use std::fmt;
use std::time::Duration;

use pip_controller::{DispatchError, schedule_claimed_dispatch};
use pip_hermes::{
    CommandRunner, GateError, GateProjectionResult, GateReleaseResult, HermesError,
    HermesGateController, HermesProjector, HermesReader, ProcessRunner, ProjectionError,
    ProjectionResult, TaskSnapshot,
};
use pip_store::{ApplyResult, Store, StoreError, TaskProjectionInput};
use serde::Serialize;

use crate::{PolicyError, RepositoryPolicy};

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
    Projected {
        effect_id: String,
        projection_count: usize,
        released_gate_count: usize,
        ledger_result: String,
    },
}

#[derive(Debug)]
pub enum DispatchCycleError {
    Store(StoreError),
    Policy(PolicyError),
    Schedule(DispatchError),
    Hermes(HermesError),
    Gate(GateError),
    Projection(ProjectionError),
    Serialization(String),
    DispatchPaused,
}

impl fmt::Display for DispatchCycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Policy(error) => error.fmt(formatter),
            Self::Schedule(error) => error.fmt(formatter),
            Self::Hermes(error) => error.fmt(formatter),
            Self::Gate(error) => error.fmt(formatter),
            Self::Projection(error) => error.fmt(formatter),
            Self::Serialization(error) => {
                write!(formatter, "dispatch serialization failed: {error}")
            }
            Self::DispatchPaused => {
                formatter.write_str("repository dispatch is disabled or paused")
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

error_from!(StoreError, Store);
error_from!(PolicyError, Policy);
error_from!(DispatchError, Schedule);
error_from!(HermesError, Hermes);
error_from!(GateError, Gate);
error_from!(ProjectionError, Projection);

pub fn dispatch_once(
    store: &mut Store,
    policy: &RepositoryPolicy,
    hermes_program: &str,
    owner: &str,
    now: u64,
    lease_seconds: u64,
) -> Result<DispatchCycleResult, DispatchCycleError> {
    dispatch_once_with(
        store,
        policy,
        ProcessRunner,
        hermes_program,
        owner,
        now,
        lease_seconds,
    )
}

pub fn dispatch_once_with<R: CommandRunner + Clone>(
    store: &mut Store,
    policy: &RepositoryPolicy,
    runner: R,
    hermes_program: &str,
    owner: &str,
    now: u64,
    lease_seconds: u64,
) -> Result<DispatchCycleResult, DispatchCycleError> {
    if !policy.dispatch_enabled || policy.intake.paused {
        return Err(DispatchCycleError::DispatchPaused);
    }
    let Some(claimed) =
        store.claim_effect_matching(owner, now, lease_seconds, &DISPATCH_EFFECTS)?
    else {
        return Ok(DispatchCycleResult::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or_else(|| StoreError::MissingCase(claimed.case_key.clone()))?;
    let dispatches = schedule_claimed_dispatch(&claimed, &case, &policy.workflow_policy()?)?;
    let timeout = Duration::from_secs(20);
    let output_bound = 4 * 1024 * 1024;
    let reader = HermesReader::new(runner.clone(), hermes_program, timeout, output_bound)?;
    let gate_controller =
        HermesGateController::new(runner.clone(), hermes_program, timeout, output_bound)?;
    let projector = HermesProjector::new(runner, hermes_program, timeout, output_bound)?;
    let mut observed = reader.list_tasks(&policy.board)?;
    let mut projections = Vec::with_capacity(dispatches.len() * 2);
    let mut gates = Vec::with_capacity(dispatches.len());
    for dispatch in dispatches {
        let gate_id = match gate_controller.project(&dispatch.gate, &observed)? {
            GateProjectionResult::Created(id) | GateProjectionResult::Existing(id) => id,
        };
        let gate = reader.show_task(&policy.board, &gate_id)?;
        upsert_observed(&mut observed, gate.clone());
        let worker_spec = dispatch.bind_gate(&gate_id)?;
        let worker_id = match projector.project(&worker_spec, &observed)? {
            ProjectionResult::Created(id) | ProjectionResult::Existing(id) => id,
        };
        let worker = reader.show_task(&policy.board, &worker_id)?;
        upsert_observed(&mut observed, worker.clone());
        projections.push(projection(
            &dispatch.gate.projection_key,
            &claimed.effect_id,
            &policy.board,
            &gate,
            &dispatch.gate,
        )?);
        projections.push(projection(
            &worker_spec.projection_key,
            &claimed.effect_id,
            &policy.board,
            &worker,
            &worker_spec,
        )?);
        gates.push((dispatch.gate, gate));
    }
    let mut released = 0;
    for (gate_spec, gate) in &gates {
        if gate_controller.release(
            gate_spec,
            gate,
            &format!("pip-controller accepted {}", claimed.effect_id),
        )? == GateReleaseResult::Released
        {
            released += 1;
        }
    }
    let ledger = store.complete_task_projections(&projections, owner, now, None)?;
    Ok(DispatchCycleResult::Projected {
        effect_id: claimed.effect_id,
        projection_count: projections.len(),
        released_gate_count: released,
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

fn upsert_observed(tasks: &mut Vec<TaskSnapshot>, task: TaskSnapshot) {
    if let Some(existing) = tasks.iter_mut().find(|existing| existing.id == task.id) {
        *existing = task;
    } else {
        tasks.push(task);
    }
}
