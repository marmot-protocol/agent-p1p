//! Hermes completion ingestion bound to controller-owned task projections.

use std::fmt;
use std::time::Duration;

use pip_contracts::{CaseIdentity, ReviewMode, WorkerBinding, WorkerResult, WorkerRole};
use pip_controller::{IngestError, IngestResult, ingest_worker_result_with_policy};
use pip_hermes::{
    CommandRunner, HermesError, HermesProjector, HermesReader, ProcessRunner, ProjectionResult,
    TaskCreateSpec,
};
use pip_store::{ProjectionRejection, Store, StoreError, StoredCase, TaskProjectionInput};
use serde::Serialize;

use crate::RepositoryPolicy;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ResultCycle {
    Idle,
    Retained {
        task_id: String,
    },
    Ingested {
        task_id: String,
        transition_count: u32,
    },
    ProviderFailureEscalated {
        task_id: String,
    },
    /// The result was rejected; a fresh task for the same job is scheduled.
    RetryScheduled {
        task_id: String,
        retry_projection: String,
        attempt: u32,
    },
    /// The worker blocked its own task; the case is parked with its reason.
    WorkerBlocked {
        task_id: String,
    },
}

/// Delay before a rejected Hermes job is projected again.
pub const RETRY_COOLDOWN_SECONDS: u64 = 120;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum RetryProjectionCycle {
    Idle,
    Blocked,
    Projected {
        projection_id: String,
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
    Projection(String),
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
            Self::Projection(error) => write!(formatter, "Hermes retry projection failed: {error}"),
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

pub fn ingest_completed_once<'a>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    hermes_program: &str,
    observed_at: u64,
) -> Result<ResultCycle, ResultCycleError> {
    ingest_completed_once_with(
        store,
        scope,
        ProcessRunner::default(),
        hermes_program,
        observed_at,
    )
}

pub fn ingest_completed_once_with<'a, R: CommandRunner>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    runner: R,
    hermes_program: &str,
    observed_at: u64,
) -> Result<ResultCycle, ResultCycleError> {
    reconcile_completed_once_with(store, scope, runner, hermes_program, observed_at, true)
}

pub fn reconcile_completed_once_with<'a, R: CommandRunner>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    runner: R,
    hermes_program: &str,
    observed_at: u64,
    advance: bool,
) -> Result<ResultCycle, ResultCycleError> {
    let scope = scope.into();
    let policy = scope.policy;
    let reader = HermesReader::new(
        runner,
        hermes_program,
        Duration::from_secs(20),
        4 * 1024 * 1024,
    )?;
    for projection in store.unconsumed_task_projections_in(policy.repository.id, scope.case_key)? {
        if projection.board != policy.board {
            continue;
        }
        if projection.desired.get("assignee").is_none() {
            continue;
        }
        let desired: TaskCreateSpec = serde_json::from_value(projection.desired.clone())
            .map_err(|_| ResultCycleError::InvalidProjection)?;
        let desired_body = desired
            .body
            .as_object()
            .ok_or(ResultCycleError::InvalidProjection)?;
        let case_key = text(desired_body, "case_key")?;
        let case = store
            .case(case_key)?
            .ok_or(ResultCycleError::InvalidProjection)?;
        if advance {
            let frozen_revision = number(desired_body, "state_revision")?;
            if !current_job_generation(
                store,
                &case,
                frozen_revision,
                matches!(
                    desired_body.get("role").and_then(|value| value.as_str()),
                    Some("reviewer-general" | "reviewer-secperf")
                ),
            )? {
                continue;
            }
        }
        // Collection is evidence preservation, not authorization under today's
        // settings. Old completions keep their accepted model/profile policy.
        let saved_policy = if !advance {
            let value = store.accepted_policy_at_case_revision(
                case_key,
                number(desired_body, "state_revision")?,
            )?;
            Some(
                crate::load_repository_policy(
                    &serde_json::to_vec(&value).map_err(|_| ResultCycleError::InvalidProjection)?,
                )
                .map_err(|_| ResultCycleError::InvalidProjection)?,
            )
        } else {
            None
        };
        validate_projection(
            &projection.task_id,
            &desired,
            saved_policy.as_ref().unwrap_or(policy),
        )?;
        let retained = store.retained_task_result(&projection.task_id)?;
        if retained.is_some() && !advance {
            continue;
        }
        let metadata = if let Some(retained) = retained {
            Ok(retained)
        } else {
            let completed = match reader.show_completed_result(&policy.board, &projection.task_id) {
                Ok(completed) => completed,
                Err(HermesError::IncompleteTask | HermesError::IncompleteRun) => continue,
                // Collection is not a workflow decision, including rejection.
                Err(HermesError::RetryLimitReached | HermesError::WorkerBlocked(_)) if !advance => {
                    continue;
                }
                Err(HermesError::RetryLimitReached) => {
                    return reject(
                        store,
                        policy,
                        &projection,
                        &desired,
                        case_key,
                        observed_at,
                        Rejection::Failed(
                            "the Hermes worker crashed, timed out, or exhausted its attempts"
                                .into(),
                        ),
                    );
                }
                Err(HermesError::WorkerBlocked(reason)) => {
                    return reject(
                        store,
                        policy,
                        &projection,
                        &desired,
                        case_key,
                        observed_at,
                        Rejection::Blocked(reason),
                    );
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
            completed
                .worker_contract_metadata()
                .map_err(|error| error.to_string())
        };
        let binding = binding(&projection.task_id, &desired)?;
        let result =
            match metadata.and_then(|metadata| validated_result(metadata, &binding, &desired)) {
                Ok(result) => result,
                // While collection is paused, leave the task for an authorized pass.
                Err(_) if !advance => continue,
                Err(reason) => {
                    return reject(
                        store,
                        policy,
                        &projection,
                        &desired,
                        case_key,
                        observed_at,
                        Rejection::Failed(format!("the worker result was invalid: {reason}")),
                    );
                }
            };
        if !advance {
            let value = serde_json::to_value(&result)
                .map_err(|error| ResultCycleError::MalformedResult(error.to_string()))?;
            store.retain_task_result(&projection.task_id, &value, observed_at)?;
            return Ok(ResultCycle::Retained {
                task_id: projection.task_id,
            });
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

fn validated_result(
    metadata: serde_json::Value,
    binding: &WorkerBinding,
    desired: &TaskCreateSpec,
) -> Result<WorkerResult, String> {
    let result = WorkerResult::decode(metadata).map_err(|error| error.to_string())?;
    result
        .validate_binding(binding)
        .map_err(|error| error.to_string())?;
    if desired.body.get("storage").is_some()
        && let WorkerResult::Planner(plan) = &result
        && !plan
            .common
            .evidence
            .get("plan_markdown")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.trim().is_empty() && text.len() <= 16 * 1024)
    {
        return Err(
            "managed-storage planners require a nonempty inline plan of at most 16 KiB".into(),
        );
    }
    Ok(result)
}

enum Rejection {
    /// Retryable: the job is projected again until the stage budget is spent.
    Failed(String),
    /// The worker stopped the job itself; retrying cannot change its reason.
    Blocked(String),
}

/// Rejects one projection's result without touching peer jobs. A failure is
/// retried with a fresh Hermes task; a worker block or a spent budget parks
/// the case with the reason so a human can resume it.
fn reject(
    store: &mut Store,
    policy: &RepositoryPolicy,
    projection: &TaskProjectionInput,
    desired: &TaskCreateSpec,
    case_key: &str,
    now: u64,
    rejection: Rejection,
) -> Result<ResultCycle, ResultCycleError> {
    let configured = policy
        .workflow_policy()
        .map_err(|_| ResultCycleError::InvalidProjection)?
        .roles()
        .iter()
        .find(|configured| configured.profile == desired.assignee)
        .cloned()
        .ok_or(ResultCycleError::InvalidProjection)?;
    let (reason, budget) = match &rejection {
        Rejection::Failed(reason) => (reason.clone(), policy.max_provider_failures),
        Rejection::Blocked(reason) => (format!("worker blocked the task: {reason}"), 1),
    };
    let outcome = store.reject_task_projection(
        &projection.projection_id,
        now,
        &reason,
        budget,
        RETRY_COOLDOWN_SECONDS,
    )?;
    let attempts = match outcome {
        ProjectionRejection::Retry {
            projection_id,
            attempt,
        } => {
            return Ok(ResultCycle::RetryScheduled {
                task_id: projection.task_id.clone(),
                retry_projection: projection_id,
                attempt,
            });
        }
        ProjectionRejection::Exhausted { attempts } => attempts,
    };
    let blocked = matches!(rejection, Rejection::Blocked(_));
    crate::bounds::escalate_case_for_bound(
        store,
        policy,
        case_key,
        now,
        crate::bounds::BoundObservation {
            bound: if blocked {
                crate::OperationalBound::WorkerBlocked
            } else {
                crate::OperationalBound::ProviderFailures
            },
            observed: u64::from(attempts),
            limit: u64::from(budget),
            details: serde_json::json!({
                "source": "hermes",
                "task_id": projection.task_id,
                "role": desired.body.get("role"),
                "profile": desired.assignee,
                "provider": configured.provider,
                "model": configured.model,
                "error": reason,
            }),
        },
    )
    .map_err(|error| ResultCycleError::MalformedResult(error.to_string()))?;
    Ok(if blocked {
        ResultCycle::WorkerBlocked {
            task_id: projection.task_id.clone(),
        }
    } else {
        ResultCycle::ProviderFailureEscalated {
            task_id: projection.task_id.clone(),
        }
    })
}

/// Creates the Hermes task for one due retry projection of a current job.
pub fn project_task_retries<'a, R: CommandRunner + Clone>(
    store: &mut Store,
    scope: impl Into<crate::RepositoryScope<'a>>,
    runner: R,
    hermes_program: &str,
    now: u64,
    authorized: bool,
) -> Result<RetryProjectionCycle, ResultCycleError> {
    if !authorized {
        return Ok(RetryProjectionCycle::Blocked);
    }
    let scope = scope.into();
    let policy = scope.policy;
    for pending in store.pending_projection_retries(policy.repository.id, scope.case_key, now)? {
        if pending.board != policy.board {
            continue;
        }
        let spec: TaskCreateSpec = serde_json::from_value(pending.desired)
            .map_err(|_| ResultCycleError::InvalidProjection)?;
        let body = spec
            .body
            .as_object()
            .ok_or(ResultCycleError::InvalidProjection)?;
        let case = store
            .case(text(body, "case_key")?)?
            .ok_or(ResultCycleError::InvalidProjection)?;
        let reviewer = matches!(
            body.get("role").and_then(|value| value.as_str()),
            Some("reviewer-general" | "reviewer-secperf")
        );
        // A retry for a job the case has moved past is never created.
        if !current_job_generation(store, &case, number(body, "state_revision")?, reviewer)? {
            continue;
        }
        let timeout = Duration::from_secs(20);
        let bound = 4 * 1024 * 1024;
        let projector = HermesProjector::new(runner.clone(), hermes_program, timeout, bound)
            .map_err(|error| ResultCycleError::Projection(error.to_string()))?;
        // The retry's own idempotency key makes a create after a crash return
        // the task that was already created rather than a duplicate.
        let task_id = match projector
            .project(&spec, &[])
            .map_err(|error| ResultCycleError::Projection(error.to_string()))?
        {
            ProjectionResult::Created(id) | ProjectionResult::Existing(id) => id,
        };
        let reader = HermesReader::new(runner.clone(), hermes_program, timeout, bound)?;
        let detail = reader.show_task_detail(&policy.board, &task_id)?;
        if projector
            .reconcile(&spec, std::slice::from_ref(&detail.task))
            .map_err(|error| ResultCycleError::Projection(error.to_string()))?
            .as_deref()
            != Some(task_id.as_str())
        {
            return Err(ResultCycleError::Projection(
                "created retry task differs from its projection".into(),
            ));
        }
        store.record_projection_task(
            &pending.projection_id,
            &task_id,
            &serde_json::to_value(&detail.task)
                .map_err(|error| ResultCycleError::Projection(error.to_string()))?,
            now,
        )?;
        return Ok(RetryProjectionCycle::Projected {
            projection_id: pending.projection_id,
            task_id,
        });
    }
    Ok(RetryProjectionCycle::Idle)
}

/// A peer review advances the ledger revision, not the job generation. Shared
/// by both adapters so retries and same-head replans fence their results alike.
pub(crate) fn current_job_generation(
    store: &Store,
    case: &StoredCase,
    frozen_revision: u64,
    reviewer: bool,
) -> Result<bool, StoreError> {
    if matches!(
        case.state.as_str(),
        "ESCALATED" | "BLOCKED" | "ABANDONED" | "COMPLETED" | "TAKEN_OVER"
    ) {
        return Ok(false);
    }
    Ok(case.state_revision == frozen_revision
        || (reviewer
            && case.state == "REVIEWING"
            && frozen_revision < case.state_revision
            && store
                .immutable_history_for_case(&case.case_key)?
                .events
                .iter()
                .filter(|event| event.state_revision > frozen_revision)
                .all(|event| event.event_type == "REVIEW_RECORDED")))
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
