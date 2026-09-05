//! Hermes completion ingestion bound to controller-owned task projections.

use std::fmt;
use std::time::Duration;

use pip_contracts::{CaseIdentity, ReviewMode, WorkerBinding, WorkerResult, WorkerRole};
use pip_controller::{IngestError, IngestResult, ingest_worker_result_with_policy};
use pip_hermes::{CommandRunner, HermesError, HermesReader, ProcessRunner, TaskCreateSpec};
use pip_store::{Store, StoreError};
use serde::Serialize;

use crate::RepositoryPolicy;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ResultCycle {
    Idle,
    Ingested {
        task_id: String,
        transition_count: u32,
    },
    ProviderFailureEscalated {
        task_id: String,
    },
}

#[derive(Debug)]
pub enum ResultCycleError {
    Store(StoreError),
    Hermes(HermesError),
    Ingest(IngestError),
    InvalidProjection,
    ProfileMismatch,
    MalformedResult(String),
}

impl fmt::Display for ResultCycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Hermes(error) => error.fmt(formatter),
            Self::Ingest(error) => error.fmt(formatter),
            Self::InvalidProjection => {
                formatter.write_str("stored worker projection has an invalid immutable binding")
            }
            Self::ProfileMismatch => {
                formatter.write_str("Hermes run profile differs from the stored projection")
            }
            Self::MalformedResult(error) => write!(formatter, "malformed worker result: {error}"),
        }
    }
}

impl std::error::Error for ResultCycleError {}

impl From<StoreError> for ResultCycleError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<HermesError> for ResultCycleError {
    fn from(error: HermesError) -> Self {
        Self::Hermes(error)
    }
}

impl From<IngestError> for ResultCycleError {
    fn from(error: IngestError) -> Self {
        Self::Ingest(error)
    }
}

pub fn ingest_completed_once(
    store: &mut Store,
    policy: &RepositoryPolicy,
    hermes_program: &str,
    observed_at: u64,
) -> Result<ResultCycle, ResultCycleError> {
    ingest_completed_once_with(
        store,
        policy,
        ProcessRunner::default(),
        hermes_program,
        observed_at,
    )
}

pub fn ingest_completed_once_with<R: CommandRunner>(
    store: &mut Store,
    policy: &RepositoryPolicy,
    runner: R,
    hermes_program: &str,
    observed_at: u64,
) -> Result<ResultCycle, ResultCycleError> {
    let reader = HermesReader::new(
        runner,
        hermes_program,
        Duration::from_secs(20),
        4 * 1024 * 1024,
    )?;
    for projection in store.unconsumed_task_projections()? {
        if projection.board != policy.board {
            continue;
        }
        if projection.desired.get("assignee").is_none() {
            continue;
        }
        let desired: TaskCreateSpec = serde_json::from_value(projection.desired.clone())
            .map_err(|_| ResultCycleError::InvalidProjection)?;
        validate_projection(&projection.task_id, &desired, policy)?;
        let desired_body = desired
            .body
            .as_object()
            .ok_or(ResultCycleError::InvalidProjection)?;
        let case_key = text(desired_body, "case_key")?;
        let case = store
            .case(case_key)?
            .ok_or(ResultCycleError::InvalidProjection)?;
        if case.state_revision != number(desired_body, "state_revision")?
            || matches!(
                case.state.as_str(),
                "ESCALATED" | "BLOCKED" | "ABANDONED" | "COMPLETED" | "TAKEN_OVER"
            )
        {
            continue;
        }
        let completed = match reader.show_completed_result(&policy.board, &projection.task_id) {
            Ok(completed) => completed,
            Err(HermesError::IncompleteTask | HermesError::IncompleteRun) => continue,
            Err(HermesError::RetryLimitReached) => {
                let configured = policy
                    .workflow_policy()
                    .map_err(|_| ResultCycleError::InvalidProjection)?
                    .roles()
                    .iter()
                    .find(|configured| configured.profile == desired.assignee)
                    .cloned()
                    .ok_or(ResultCycleError::InvalidProjection)?;
                crate::bounds::escalate_case_for_bound(
                    store,
                    policy,
                    case_key,
                    observed_at,
                    crate::bounds::BoundObservation {
                        bound: crate::OperationalBound::ProviderFailures,
                        observed: u64::from(desired.max_retries),
                        limit: u64::from(desired.max_retries),
                        details: serde_json::json!({
                            "source": "hermes-circuit-breaker",
                            "task_id": projection.task_id,
                            "profile": desired.assignee,
                            "provider": configured.provider,
                            "model": configured.model,
                        }),
                    },
                )
                .map_err(|error| ResultCycleError::MalformedResult(error.to_string()))?;
                return Ok(ResultCycle::ProviderFailureEscalated {
                    task_id: projection.task_id,
                });
            }
            Err(error) => return Err(error.into()),
        };
        if completed.profile != desired.assignee
            || completed.task.assignee.as_deref() != Some(desired.assignee.as_str())
            || completed.task.created_by.as_deref() != Some("pip-controller")
            || completed.task.title != desired.title
            || projection_key(&completed.task.body).as_deref()
                != Some(desired.projection_key.as_str())
        {
            return Err(ResultCycleError::ProfileMismatch);
        }
        let binding = binding(&projection.task_id, &desired)?;
        let result: WorkerResult = serde_json::from_value(completed.worker_contract_metadata()?)
            .map_err(|error| ResultCycleError::MalformedResult(error.to_string()))?;
        if desired.body.get("storage").is_some()
            && let WorkerResult::Planner(plan) = &result
            && !plan
                .common
                .evidence
                .get("plan_markdown")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|text| !text.trim().is_empty() && text.len() <= 16 * 1024)
        {
            return Err(ResultCycleError::MalformedResult(
                "managed-storage planners require a nonempty inline plan of at most 16 KiB".into(),
            ));
        }
        let workflow_policy = policy
            .workflow_policy()
            .map_err(|error| ResultCycleError::MalformedResult(error.to_string()))?;
        let ingested = ingest_worker_result_with_policy(
            store,
            &policy.case_policy(),
            &workflow_policy,
            &binding,
            &result,
        )?;
        return Ok(match ingested {
            IngestResult::Applied { transition_count } => ResultCycle::Ingested {
                task_id: projection.task_id,
                transition_count,
            },
            IngestResult::Replayed => ResultCycle::Idle,
        });
    }
    Ok(ResultCycle::Idle)
}

fn validate_projection(
    task_id: &str,
    desired: &TaskCreateSpec,
    policy: &RepositoryPolicy,
) -> Result<(), ResultCycleError> {
    let body = desired
        .body
        .as_object()
        .ok_or(ResultCycleError::InvalidProjection)?;
    let role = role(body.get("role").and_then(|value| value.as_str()))?;
    let workflow = policy
        .workflow_policy()
        .map_err(|_| ResultCycleError::InvalidProjection)?;
    let reviewer_id = body.get("reviewer_id").and_then(|value| value.as_str());
    let configured = if let Some(reviewer_id) = reviewer_id {
        workflow
            .reviewer(reviewer_id)
            .map_err(|_| ResultCycleError::InvalidProjection)?
    } else {
        workflow
            .roles()
            .iter()
            .find(|configured| configured.role == role && configured.reviewer_id.is_none())
            .ok_or(ResultCycleError::InvalidProjection)?
    };
    if task_id.trim().is_empty()
        || desired.board != policy.board
        || number(body, "repository_id")? != policy.repository.id
        || number(body, "workflow_version")? != u64::from(policy.workflow_version)
        || desired.assignee != configured.profile
        || desired.provider != configured.provider
        || desired.model != configured.model
        || desired.skills != configured.skills
        || reviewer_id != configured.reviewer_id.as_deref()
        || body.get("review_mode").and_then(|value| value.as_str())
            != configured.review_mode.map(|mode| match mode {
                ReviewMode::Required => "required",
                ReviewMode::Advisory => "advisory",
                ReviewMode::Shadow => "shadow",
            })
        || desired
            .body
            .get("requested_model")
            .and_then(|value| value.as_str())
            != Some(format!("{}/{}", configured.provider, configured.model).as_str())
    {
        return Err(ResultCycleError::InvalidProjection);
    }
    Ok(())
}

fn binding(task_id: &str, desired: &TaskCreateSpec) -> Result<WorkerBinding, ResultCycleError> {
    let body = desired
        .body
        .as_object()
        .ok_or(ResultCycleError::InvalidProjection)?;
    Ok(WorkerBinding {
        case: CaseIdentity {
            repository_id: number(body, "repository_id")?,
            issue_number: number(body, "issue_number")?,
            workflow_version: u32::try_from(number(body, "workflow_version")?)
                .map_err(|_| ResultCycleError::InvalidProjection)?,
        },
        task_id: task_id.into(),
        role: role(body.get("role").and_then(|value| value.as_str()))?,
        reviewer_id: body
            .get("reviewer_id")
            .map(|_| text(body, "reviewer_id").map(str::to_owned))
            .transpose()?,
        review_mode: body
            .get("review_mode")
            .map(|_| review_mode(text(body, "review_mode")?))
            .transpose()?,
        requested_model: text(body, "requested_model")?.into(),
        skills_repository_commit: text(body, "skills_repository_commit")?.into(),
        plan_version: u32::try_from(number(body, "plan_version")?)
            .map_err(|_| ResultCycleError::InvalidProjection)?,
        pr_number: optional_number(body, "pr_number")?,
        expected_head_sha: body
            .get("expected_head_sha")
            .map(|_| text(body, "expected_head_sha").map(str::to_owned))
            .transpose()?,
    })
}

fn review_mode(value: &str) -> Result<ReviewMode, ResultCycleError> {
    match value {
        "required" => Ok(ReviewMode::Required),
        "advisory" => Ok(ReviewMode::Advisory),
        "shadow" => Ok(ReviewMode::Shadow),
        _ => Err(ResultCycleError::InvalidProjection),
    }
}

fn role(value: Option<&str>) -> Result<WorkerRole, ResultCycleError> {
    match value {
        Some("planner") => Ok(WorkerRole::Planner),
        Some("builder") => Ok(WorkerRole::Builder),
        Some("reviewer-general") => Ok(WorkerRole::ReviewerGeneral),
        Some("reviewer-secperf") => Ok(WorkerRole::ReviewerSecperf),
        Some("final-reviewer") => Ok(WorkerRole::FinalReviewer),
        _ => Err(ResultCycleError::InvalidProjection),
    }
}

fn number(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<u64, ResultCycleError> {
    body.get(field)
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or(ResultCycleError::InvalidProjection)
}

fn optional_number(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<u64>, ResultCycleError> {
    body.get(field).map(|_| number(body, field)).transpose()
}

fn text<'a>(
    body: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str, ResultCycleError> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ResultCycleError::InvalidProjection)
}

fn projection_key(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("projection_key")?
        .as_str()
        .map(str::to_owned)
}
