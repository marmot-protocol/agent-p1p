//! Durable execution cycle for roles bound to a direct provider runtime.

use std::fmt;

use pip_contracts::{CaseIdentity, WorkerBinding, WorkerResult, WorkerRole};
use pip_controller::{
    DirectTaskSpec, ExecutionKind, IngestError, IngestResult, ingest_worker_result,
};
use pip_store::{ClaimedEffect, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::Map;

use crate::{PolicyError, RepositoryPolicy};

const RUN_EFFECT: &str = "RUN_DIRECT_WORKER";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DirectWorkerCycle {
    Idle,
    AuthorizationBlocked,
    Ingested {
        task_id: String,
        transition_count: u32,
    },
    Replayed {
        task_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectWorkerCycleContext<'a> {
    pub owner: &'a str,
    pub now: u64,
    pub lease_seconds: u64,
    pub authorization_valid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectWorkerRuntimeError {
    Unavailable(String),
}

impl fmt::Display for DirectWorkerRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => {
                write!(formatter, "direct worker runtime unavailable: {error}")
            }
        }
    }
}

impl std::error::Error for DirectWorkerRuntimeError {}

pub trait DirectWorkerRuntime {
    fn execute(&self, task: &DirectTaskSpec) -> Result<WorkerResult, DirectWorkerRuntimeError>;
}

#[derive(Debug)]
pub enum DirectWorkerError {
    Store(StoreError),
    Policy(PolicyError),
    Ingest(IngestError),
    Runtime(DirectWorkerRuntimeError),
    DispatchPaused,
    InvalidJob,
}

impl fmt::Display for DirectWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Policy(error) => error.fmt(formatter),
            Self::Ingest(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::DispatchPaused => {
                formatter.write_str("repository dispatch is disabled or paused")
            }
            Self::InvalidJob => {
                formatter.write_str("direct worker job has an invalid immutable binding")
            }
        }
    }
}

impl std::error::Error for DirectWorkerError {}

impl From<StoreError> for DirectWorkerError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<PolicyError> for DirectWorkerError {
    fn from(error: PolicyError) -> Self {
        Self::Policy(error)
    }
}

impl From<IngestError> for DirectWorkerError {
    fn from(error: IngestError) -> Self {
        Self::Ingest(error)
    }
}

pub fn run_direct_worker_once_with<R: DirectWorkerRuntime>(
    store: &mut Store,
    policy: &RepositoryPolicy,
    runtime: &R,
    context: DirectWorkerCycleContext<'_>,
) -> Result<DirectWorkerCycle, DirectWorkerError> {
    if !policy.dispatch_enabled || policy.intake.paused {
        return Err(DirectWorkerError::DispatchPaused);
    }
    if !context.authorization_valid {
        return Ok(DirectWorkerCycle::AuthorizationBlocked);
    }
    let Some(claimed) = store.claim_effect_matching(
        context.owner,
        context.now,
        context.lease_seconds,
        &[RUN_EFFECT],
    )?
    else {
        return Ok(DirectWorkerCycle::Idle);
    };

    let processed = process_claimed(store, policy, runtime, &claimed);
    match processed {
        Ok((task_id, IngestResult::Applied { transition_count })) => {
            Ok(DirectWorkerCycle::Ingested {
                task_id,
                transition_count,
            })
        }
        Ok((task_id, IngestResult::Replayed)) => {
            store.acknowledge_effect(&claimed.effect_id, context.owner, context.now)?;
            Ok(DirectWorkerCycle::Replayed { task_id })
        }
        Err(error) => {
            store.release_effect(&claimed.effect_id, context.owner)?;
            Err(error)
        }
    }
}

fn process_claimed<R: DirectWorkerRuntime>(
    store: &mut Store,
    policy: &RepositoryPolicy,
    runtime: &R,
    claimed: &ClaimedEffect,
) -> Result<(String, IngestResult), DirectWorkerError> {
    let task: DirectTaskSpec = serde_json::from_value(claimed.payload.clone())
        .map_err(|_| DirectWorkerError::InvalidJob)?;
    let case = store
        .case(&claimed.case_key)?
        .ok_or(DirectWorkerError::InvalidJob)?;
    let binding = validate_job(claimed, &case, &task, policy)?;
    let result = runtime.execute(&task).map_err(DirectWorkerError::Runtime)?;
    let ingested = ingest_worker_result(store, &policy.case_policy(), &binding, &result)?;
    Ok((task.task_id, ingested))
}

fn validate_job(
    claimed: &ClaimedEffect,
    case: &StoredCase,
    task: &DirectTaskSpec,
    policy: &RepositoryPolicy,
) -> Result<WorkerBinding, DirectWorkerError> {
    let body = task.body.as_object().ok_or(DirectWorkerError::InvalidJob)?;
    let workflow = policy.workflow_policy()?;
    let configured = workflow
        .roles()
        .iter()
        .find(|configured| configured.role == task.role)
        .ok_or(DirectWorkerError::InvalidJob)?;
    let role = role_name(task.role);
    let expected_effect_id = format!("{}:direct:{role}", task.source_effect_id);
    let expected_workspace = format!(
        "{}/repo-{}-issue-{}-workflow-{}",
        policy.workspace.trim_end_matches('/'),
        case.repository_id,
        case.issue_number,
        case.workflow_version
    );
    let round = match task.role {
        WorkerRole::Planner => case.plan_version.saturating_add(1).max(1),
        WorkerRole::Builder => case.remediation_round.max(1),
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf | WorkerRole::FinalReviewer => {
            case.remediation_round.saturating_add(1)
        }
    };
    let expected_task_id = format!(
        "{}:{role}:round:{round}:revision:{}:worker",
        case.case_key, case.state_revision
    );
    let requested_model = format!("{}/{}", configured.provider, configured.model);
    let invalid_model = matches!(
        task.model.to_ascii_lowercase().as_str(),
        "auto" | "default" | "latest"
    );
    let valid = task.schema_version == 1
        && claimed.effect_type == RUN_EFFECT
        && claimed.effect_id == expected_effect_id
        && claimed.case_key == case.case_key
        && claimed.state_revision == case.state_revision
        && case.repository_id == policy.repository.id
        && case.workflow_version == policy.workflow_version
        && task.task_id == expected_task_id
        && task.title == format!("Run {role} for {}", case.case_key)
        && task.workspace == expected_workspace
        && configured.execution == ExecutionKind::Direct
        && task.profile == configured.profile
        && task.provider == configured.provider
        && task.model == configured.model
        && !invalid_model
        && task.skills == configured.skills
        && task.max_runtime == configured.max_runtime
        && task.priority == configured.priority
        && task.provider == "cursor"
        && text(body, "case_key")? == case.case_key
        && number(body, "repository_id")? == case.repository_id
        && number(body, "issue_number")? == case.issue_number
        && number(body, "workflow_version")? == u64::from(case.workflow_version)
        && number(body, "state_revision")? == case.state_revision
        && text(body, "role")? == role
        && text(body, "execution")? == "direct"
        && text(body, "provider")? == configured.provider
        && text(body, "model")? == configured.model
        && text(body, "requested_model")? == requested_model
        && valid_hex(text(body, "skills_repository_commit")?, 40)
        && valid_evidence_bundle(body.get("immutable_evidence_bundle"));
    if !valid {
        return Err(DirectWorkerError::InvalidJob);
    }
    validate_role_fields(body, task.role, case, policy, &expected_workspace)?;

    Ok(WorkerBinding {
        case: CaseIdentity {
            repository_id: case.repository_id,
            issue_number: case.issue_number,
            workflow_version: case.workflow_version,
        },
        task_id: task.task_id.clone(),
        role: task.role,
        requested_model,
        skills_repository_commit: text(body, "skills_repository_commit")?.into(),
        plan_version: u32::try_from(number(body, "plan_version")?)
            .map_err(|_| DirectWorkerError::InvalidJob)?,
        pr_number: optional_number(body, "pr_number")?,
        expected_head_sha: optional_text(body, "expected_head_sha")?.map(str::to_owned),
    })
}

fn validate_role_fields(
    body: &Map<String, serde_json::Value>,
    role: WorkerRole,
    case: &StoredCase,
    policy: &RepositoryPolicy,
    expected_workspace: &str,
) -> Result<(), DirectWorkerError> {
    let expected_plan = if role == WorkerRole::Planner {
        case.plan_version.saturating_add(1).max(1)
    } else {
        case.plan_version
    };
    if number(body, "plan_version")? != u64::from(expected_plan)
        || optional_number(body, "pr_number")? != case.pr_number
        || optional_text(body, "expected_head_sha")? != case.head_sha.as_deref()
    {
        return Err(DirectWorkerError::InvalidJob);
    }
    if role == WorkerRole::Builder {
        let expected_branch = format!(
            "{}repo-{}/issue-{}/workflow-{}",
            policy.branch_prefix, case.repository_id, case.issue_number, case.workflow_version
        );
        if text(body, "assigned_branch")? != expected_branch
            || text(body, "assigned_worktree")? != expected_workspace
        {
            return Err(DirectWorkerError::InvalidJob);
        }
    }
    Ok(())
}

fn number(body: &Map<String, serde_json::Value>, field: &str) -> Result<u64, DirectWorkerError> {
    body.get(field)
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or(DirectWorkerError::InvalidJob)
}

fn optional_number(
    body: &Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<u64>, DirectWorkerError> {
    body.get(field).map(|_| number(body, field)).transpose()
}

fn text<'a>(
    body: &'a Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str, DirectWorkerError> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(DirectWorkerError::InvalidJob)
}

fn optional_text<'a>(
    body: &'a Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<&'a str>, DirectWorkerError> {
    body.get(field).map(|_| text(body, field)).transpose()
}

fn valid_evidence_bundle(value: Option<&serde_json::Value>) -> bool {
    let Some(bundle) = value.and_then(serde_json::Value::as_object) else {
        return false;
    };
    bundle
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        == Some(1)
        && bundle
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|digest| valid_hex(digest, 64))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn role_name(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Planner => "planner",
        WorkerRole::Builder => "builder",
        WorkerRole::ReviewerGeneral => "reviewer-general",
        WorkerRole::ReviewerSecperf => "reviewer-secperf",
        WorkerRole::FinalReviewer => "final-reviewer",
    }
}
