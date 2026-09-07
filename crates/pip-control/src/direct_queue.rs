//! Filesystem bridge between the authoritative controller and an unprivileged direct worker.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use pip_contracts::{ReviewMode, WorkerResult};
use pip_controller::{DirectTaskSpec, IngestResult, ingest_worker_result_with_policy};
use pip_store::{
    ClaimedEffect, DirectAttemptStatus, EvidenceInput, ReviewObservationInput, Store, StoreError,
    StoredCase,
};
use serde::{Deserialize, Serialize};

use crate::direct_worker::{DirectWorkerRuntime, validate_job};
use crate::{DirectWorkerRuntimeError, PolicyError, RepositoryPolicy};

const MAX_QUEUE_FILES: usize = 1024;
const MAX_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DirectQueueCycle {
    Idle,
    AuthorizationBlocked,
    Retained {
        attempt_id: u64,
    },
    Prepared {
        attempt_id: u64,
        task_id: String,
    },
    Executed {
        attempt_id: u64,
        succeeded: bool,
    },
    Ingested {
        task_id: String,
        transition_count: u32,
    },
    Observed {
        task_id: String,
    },
    Cleaned {
        attempt_id: u64,
    },
    Failed {
        attempt_id: u64,
    },
}

#[derive(Debug)]
pub enum DirectQueueError {
    Store(StoreError),
    Policy(PolicyError),
    Runtime(DirectWorkerRuntimeError),
    Ingest(String),
    InvalidQueue,
    InvalidEnvelope,
    Filesystem(String),
    Serialization(String),
}

impl fmt::Display for DirectQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Policy(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Ingest(error) => write!(formatter, "direct queue ingestion error: {error}"),
            Self::InvalidQueue => formatter.write_str("invalid direct worker queue"),
            Self::InvalidEnvelope => formatter.write_str("invalid direct worker envelope"),
            Self::Filesystem(error) => write!(formatter, "direct queue filesystem error: {error}"),
            Self::Serialization(error) => {
                write!(formatter, "direct queue serialization error: {error}")
            }
        }
    }
}

impl std::error::Error for DirectQueueError {}

impl From<StoreError> for DirectQueueError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<PolicyError> for DirectQueueError {
    fn from(error: PolicyError) -> Self {
        Self::Policy(error)
    }
}

#[derive(Clone, Debug)]
pub struct DirectQueue {
    inbox: PathBuf,
    results: PathBuf,
    archive: PathBuf,
}

impl DirectQueue {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, DirectQueueError> {
        let root = real_directory(root.as_ref())?;
        let inbox = real_directory(&root.join("inbox"))?;
        let results = real_directory(&root.join("results"))?;
        let archive = real_directory(&root.join("archive"))?;
        Ok(Self {
            inbox,
            results,
            archive,
        })
    }

    fn path(&self, directory: &Path, attempt_id: u64) -> PathBuf {
        directory.join(format!("attempt-{attempt_id}.json"))
    }

    fn input(&self, attempt_id: u64) -> PathBuf {
        self.path(&self.inbox, attempt_id)
    }
    fn result(&self, attempt_id: u64) -> PathBuf {
        self.path(&self.results, attempt_id)
    }

    fn archive(&self, attempt_id: u64) -> Result<(), DirectQueueError> {
        for (source, suffix) in [
            (self.input(attempt_id), "input"),
            (self.result(attempt_id), "result"),
        ] {
            if source.exists() {
                fs::rename(
                    &source,
                    self.archive
                        .join(format!("attempt-{attempt_id}.{suffix}.json")),
                )
                .map_err(filesystem)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkEnvelope {
    schema_version: u32,
    attempt_id: u64,
    claimed: ClaimedEffect,
    task: DirectTaskSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "status",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum ResultEnvelope {
    Unavailable {
        schema_version: u32,
        attempt_id: u64,
        error: String,
    },
    Complete {
        schema_version: u32,
        attempt_id: u64,
        result: Box<WorkerResult>,
    },
    Failed {
        schema_version: u32,
        attempt_id: u64,
        error: String,
    },
}

impl ResultEnvelope {
    const fn attempt_id(&self) -> u64 {
        match self {
            Self::Complete { attempt_id, .. }
            | Self::Failed { attempt_id, .. }
            | Self::Unavailable { attempt_id, .. } => *attempt_id,
        }
    }

    const fn schema_version(&self) -> u32 {
        match self {
            Self::Complete { schema_version, .. }
            | Self::Failed { schema_version, .. }
            | Self::Unavailable { schema_version, .. } => *schema_version,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn reconcile_direct_queue_once(
    store: &mut Store,
    policy: &RepositoryPolicy,
    queue: &DirectQueue,
    owner: &str,
    now: u64,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<DirectQueueCycle, DirectQueueError> {
    let mut retained = None;
    for path in queue_files(&queue.results)? {
        let result = ingest_result(store, policy, queue, &path, now, authorization_valid)?;
        if matches!(result, DirectQueueCycle::Retained { .. }) {
            retained = Some(result);
        } else {
            return Ok(result);
        }
    }
    if let Some(result) = retained {
        return Ok(result);
    }
    if !authorization_valid {
        return Ok(DirectQueueCycle::AuthorizationBlocked);
    }
    // This queue has one serial worker. Do not start another job's lease while
    // it can only wait, or replace an uncertain handoff after its lease expires.
    // Existing multi-job queues drain normally through result reconciliation.
    if !queue_files(&queue.inbox)?.is_empty() {
        return Ok(DirectQueueCycle::Idle);
    }
    // Comparisons use spare queue capacity, never priority over required work.
    let claimed = match store.claim_repository_effect_matching(
        policy.repository.id,
        owner,
        now,
        lease_seconds,
        &["RUN_DIRECT_WORKER"],
    )? {
        Some(claimed) => Some(claimed),
        None => store.claim_repository_effect_matching(
            policy.repository.id,
            owner,
            now,
            lease_seconds,
            &["RUN_DIRECT_OBSERVER"],
        )?,
    };
    let Some(claimed) = claimed else {
        return Ok(DirectQueueCycle::Idle);
    };
    let task: DirectTaskSpec = serde_json::from_value(claimed.payload.clone())
        .map_err(|_| DirectQueueError::InvalidEnvelope)?;
    let case = store
        .case(&claimed.case_key)?
        .ok_or(DirectQueueError::InvalidEnvelope)?;
    let validation_case = validation_case(&claimed, &case, &task, false)?;
    validate_job(&claimed, &validation_case, &task, policy)
        .map_err(|_| DirectQueueError::InvalidEnvelope)?;
    let attempt_id = store.begin_direct_attempt(&claimed, &task.task_id, now)?;
    let envelope = WorkEnvelope {
        schema_version: 1,
        attempt_id,
        claimed: claimed.clone(),
        task: task.clone(),
    };
    if let Err(error) = write_new(&queue.input(attempt_id), &envelope, 0o440) {
        // An existing or uninspectable handoff might already have a worker.
        // Only a confirmed missing message proves that execution never started.
        if matches!(fs::symlink_metadata(queue.input(attempt_id)), Err(ref e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            store.record_direct_unavailability(
                attempt_id,
                &claimed.lease_owner,
                now,
                &bounded_error(&error.to_string()),
            )?;
        }
        return Err(error);
    }
    Ok(DirectQueueCycle::Prepared {
        attempt_id,
        task_id: task.task_id,
    })
}

pub fn execute_direct_queue_once<R: DirectWorkerRuntime>(
    runtime: &R,
    queue: &DirectQueue,
    now: u64,
) -> Result<DirectQueueCycle, DirectQueueError> {
    for path in queue_files(&queue.inbox)? {
        let work: WorkEnvelope = read_envelope(&path)?;
        if work.schema_version != 1 || work.attempt_id == 0 || path != queue.input(work.attempt_id)
        {
            return Err(DirectQueueError::InvalidEnvelope);
        }
        let result_path = queue.result(work.attempt_id);
        if result_path.exists() {
            continue;
        }
        let result = if now > work.claimed.lease_until {
            ResultEnvelope::Unavailable {
                schema_version: 1,
                attempt_id: work.attempt_id,
                error: "controller lease expired before direct execution".into(),
            }
        } else {
            match runtime.execute(&work.task, work.attempt_id) {
                Ok(result) => ResultEnvelope::Complete {
                    schema_version: 1,
                    attempt_id: work.attempt_id,
                    result: Box::new(result),
                },
                Err(DirectWorkerRuntimeError::Unavailable(error)) => ResultEnvelope::Unavailable {
                    schema_version: 1,
                    attempt_id: work.attempt_id,
                    error: bounded_error(&error),
                },
                Err(error) => ResultEnvelope::Failed {
                    schema_version: 1,
                    attempt_id: work.attempt_id,
                    error: bounded_error(&error.to_string()),
                },
            }
        };
        let succeeded = matches!(result, ResultEnvelope::Complete { .. });
        write_new(&result_path, &result, 0o640)?;
        return Ok(DirectQueueCycle::Executed {
            attempt_id: work.attempt_id,
            succeeded,
        });
    }
    Ok(DirectQueueCycle::Idle)
}

fn ingest_result(
    store: &mut Store,
    policy: &RepositoryPolicy,
    queue: &DirectQueue,
    result_path: &Path,
    now: u64,
    advance: bool,
) -> Result<DirectQueueCycle, DirectQueueError> {
    let envelope: ResultEnvelope = read_envelope(result_path)?;
    let attempt_id = envelope.attempt_id();
    if envelope.schema_version() != 1 || result_path != queue.result(attempt_id) {
        return Err(DirectQueueError::InvalidEnvelope);
    }
    let attempt = store
        .direct_attempt(attempt_id)?
        .ok_or(DirectQueueError::InvalidEnvelope)?;
    let work: WorkEnvelope = read_envelope(&queue.input(attempt_id))?;
    if work.schema_version != 1
        || work.attempt_id != attempt_id
        || attempt.effect_id != work.claimed.effect_id
        || attempt.case_key != work.claimed.case_key
        || attempt.state_revision != work.claimed.state_revision
        || attempt.task_id != work.task.task_id
        || attempt.lease_owner != work.claimed.lease_owner
        || attempt.lease_until != work.claimed.lease_until
    {
        return Err(DirectQueueError::InvalidEnvelope);
    }
    if work.claimed.effect_type == "RUN_DIRECT_WORKER"
        && store.run_by_task_id(&attempt.task_id)?.is_some()
    {
        queue.archive(attempt_id)?;
        return Ok(DirectQueueCycle::Cleaned { attempt_id });
    }
    let case = store
        .case(&attempt.case_key)?
        .ok_or(DirectQueueError::InvalidEnvelope)?;
    // Retention validates the frozen job, not the current case revision or
    // today's profile settings. Workflow acceptance is separately fenced below.
    let validation_case = validation_case(&work.claimed, &case, &work.task, true)?;
    let saved_policy = crate::load_repository_policy(
        &serde_json::to_vec(&store.accepted_policy(case.repository_id, case.policy_revision)?)
            .map_err(serialization)?,
    )?;
    let binding = validate_job(&work.claimed, &validation_case, &work.task, &saved_policy)
        .map_err(|_| DirectQueueError::InvalidEnvelope)?;
    let unavailable = matches!(envelope, ResultEnvelope::Unavailable { .. });
    match envelope {
        ResultEnvelope::Failed { error, .. } | ResultEnvelope::Unavailable { error, .. } => {
            let detached_observer = work.claimed.effect_type == "RUN_DIRECT_OBSERVER";
            if unavailable && !detached_observer {
                store.record_direct_unavailability(
                    attempt_id,
                    &attempt.lease_owner,
                    now,
                    &error,
                )?;
                queue.archive(attempt_id)?;
                return Ok(DirectQueueCycle::Failed { attempt_id });
            }
            if attempt.status == DirectAttemptStatus::Running {
                store.fail_direct_attempt(
                    attempt_id,
                    &attempt.lease_owner,
                    now,
                    &bounded_error(&error),
                )?;
            }
            if detached_observer {
                if attempt.status == DirectAttemptStatus::Complete {
                    return Err(DirectQueueError::InvalidEnvelope);
                }
                store.complete_effect_evidence(
                    &attempt.effect_id,
                    &attempt.lease_owner,
                    now,
                    &EvidenceInput {
                        evidence_id: format!("evidence-observer-failed-{}", attempt.task_id),
                        kind: "DETACHED_REVIEW_FAILURE".into(),
                        source: attempt.task_id.clone(),
                        payload: serde_json::json!({"error": bounded_error(&error)}),
                    },
                )?;
            } else if attempt.status == DirectAttemptStatus::Running {
                store.release_effect(&attempt.effect_id, &attempt.lease_owner)?;
            }
            queue.archive(attempt_id)?;
            Ok(DirectQueueCycle::Failed { attempt_id })
        }
        ResultEnvelope::Complete { result, .. } => {
            // Collection is not workflow acceptance. Bind before preserving a
            // result, including during a pause; only a fresh authorization may
            // advance the case or publish the observer's disposition.
            result
                .validate_binding(&binding)
                .map_err(|error| DirectQueueError::Ingest(error.to_string()))?;
            let result = if attempt.status == DirectAttemptStatus::Complete {
                serde_json::from_value(attempt.result.ok_or(DirectQueueError::InvalidEnvelope)?)
                    .map_err(|error| DirectQueueError::Serialization(error.to_string()))?
            } else if attempt.status == DirectAttemptStatus::Running {
                store.complete_direct_attempt(
                    attempt_id,
                    &attempt.lease_owner,
                    now,
                    &serde_json::to_value(&result).map_err(serialization)?,
                )?;
                *result
            } else {
                queue.archive(attempt_id)?;
                return Ok(DirectQueueCycle::Cleaned { attempt_id });
            };
            if !advance {
                return Ok(DirectQueueCycle::Retained { attempt_id });
            }
            if work.claimed.effect_type == "RUN_DIRECT_WORKER"
                && !crate::results::current_job_generation(
                    store,
                    &case,
                    work.claimed.state_revision,
                    matches!(
                        binding.role,
                        pip_contracts::WorkerRole::ReviewerGeneral
                            | pip_contracts::WorkerRole::ReviewerSecperf
                    ),
                )?
            {
                queue.archive(attempt_id)?;
                return Ok(DirectQueueCycle::Cleaned { attempt_id });
            }
            if matches!(
                binding.review_mode,
                Some(ReviewMode::Advisory | ReviewMode::Shadow)
            ) {
                result
                    .validate_binding(&binding)
                    .map_err(|error| DirectQueueError::Ingest(error.to_string()))?;
                let WorkerResult::Review(review) = &result else {
                    return Err(DirectQueueError::InvalidEnvelope);
                };
                store.complete_review_observation_effect(
                    &attempt.effect_id,
                    &attempt.lease_owner,
                    now,
                    &ReviewObservationInput {
                        observation_id: format!("observation-{}", attempt.task_id),
                        task_id: attempt.task_id.clone(),
                        reviewer_id: review.reviewer_id.clone(),
                        role: role_name(review.common.role).into(),
                        review_mode: match binding.review_mode {
                            Some(ReviewMode::Advisory) => "advisory".into(),
                            Some(ReviewMode::Shadow) => "shadow".into(),
                            _ => return Err(DirectQueueError::InvalidEnvelope),
                        },
                        plan_version: review.plan_version,
                        review_round: review.review_round,
                        pr_number: review.pr_number,
                        reviewed_head_sha: review.reviewed_head_sha.clone(),
                        payload: serde_json::to_value(&result).map_err(serialization)?,
                    },
                )?;
                queue.archive(attempt_id)?;
                return Ok(DirectQueueCycle::Observed {
                    task_id: attempt.task_id,
                });
            }
            let workflow_policy = policy.workflow_policy()?;
            let ingested = ingest_worker_result_with_policy(
                store,
                &policy.case_policy(),
                &workflow_policy,
                &binding,
                &result,
            )
            .map_err(|error| DirectQueueError::Ingest(error.to_string()))?;
            queue.archive(attempt_id)?;
            match ingested {
                IngestResult::Applied { transition_count } => Ok(DirectQueueCycle::Ingested {
                    task_id: attempt.task_id,
                    transition_count,
                }),
                IngestResult::Replayed => Ok(DirectQueueCycle::Cleaned { attempt_id }),
            }
        }
    }
}

fn validation_case(
    claimed: &ClaimedEffect,
    current: &StoredCase,
    task: &DirectTaskSpec,
    frozen: bool,
) -> Result<StoredCase, DirectQueueError> {
    if !frozen && claimed.effect_type != "RUN_DIRECT_OBSERVER" {
        return Ok(current.clone());
    }
    let body = task
        .body
        .as_object()
        .ok_or(DirectQueueError::InvalidEnvelope)?;
    let u64_field = |name: &str| {
        body.get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or(DirectQueueError::InvalidEnvelope)
    };
    let optional_u64 = |name: &str| {
        body.get(name)
            .map(|value| value.as_u64().ok_or(DirectQueueError::InvalidEnvelope))
            .transpose()
    };
    let optional_text = |name: &str| {
        body.get(name)
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .ok_or(DirectQueueError::InvalidEnvelope)
            })
            .transpose()
    };
    if claimed.case_key != current.case_key
        || u64_field("repository_id")? != current.repository_id
        || u64_field("issue_number")? != current.issue_number
        || u64_field("workflow_version")? != u64::from(current.workflow_version)
    {
        return Err(DirectQueueError::InvalidEnvelope);
    }
    Ok(StoredCase {
        case_key: current.case_key.clone(),
        repository_id: current.repository_id,
        issue_number: current.issue_number,
        workflow_version: current.workflow_version,
        state: "REVIEWING".into(),
        state_revision: claimed.state_revision,
        policy_revision: current.policy_revision,
        remediation_round: u32::try_from(u64_field("remediation_round")?)
            .map_err(|_| DirectQueueError::InvalidEnvelope)?,
        plan_version: u32::try_from(u64_field("plan_version")?)
            .map_err(|_| DirectQueueError::InvalidEnvelope)?,
        pr_number: optional_u64("pr_number")?,
        head_sha: optional_text("expected_head_sha")?,
    })
}

fn role_name(role: pip_contracts::WorkerRole) -> &'static str {
    match role {
        pip_contracts::WorkerRole::Planner => "planner",
        pip_contracts::WorkerRole::Builder => "builder",
        pip_contracts::WorkerRole::ReviewerGeneral => "reviewer-general",
        pip_contracts::WorkerRole::ReviewerSecperf => "reviewer-secperf",
        pip_contracts::WorkerRole::FinalReviewer => "final-reviewer",
    }
}

fn queue_files(directory: &Path) -> Result<Vec<PathBuf>, DirectQueueError> {
    let mut paths = fs::read_dir(directory)
        .map_err(filesystem)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(filesystem))
        .collect::<Result<Vec<_>, _>>()?;
    if paths.len() > MAX_QUEUE_FILES {
        return Err(DirectQueueError::InvalidQueue);
    }
    paths.sort();
    if paths.iter().any(|path| {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return true;
        };
        metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_ENVELOPE_BYTES as u64
    }) {
        return Err(DirectQueueError::InvalidQueue);
    }
    Ok(paths)
}

fn read_envelope<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, DirectQueueError> {
    let metadata = fs::symlink_metadata(path).map_err(filesystem)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_ENVELOPE_BYTES as u64
    {
        return Err(DirectQueueError::InvalidEnvelope);
    }
    let mut file = fs::File::open(path).map_err(filesystem)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take((MAX_ENVELOPE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(filesystem)?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(DirectQueueError::InvalidEnvelope);
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| DirectQueueError::Serialization(error.to_string()))
}

fn write_new(path: &Path, value: &impl Serialize, mode: u32) -> Result<(), DirectQueueError> {
    let bytes = serde_json::to_vec(value).map_err(serialization)?;
    if bytes.is_empty() || bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(DirectQueueError::InvalidEnvelope);
    }
    let temporary = path.with_extension("json.new");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(filesystem)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(filesystem(error));
    }
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode)).map_err(filesystem)?;
    if let Err(error) = fs::hard_link(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(filesystem(error));
    }
    fs::remove_file(&temporary).map_err(filesystem)
}

fn real_directory(path: &Path) -> Result<PathBuf, DirectQueueError> {
    let metadata = fs::symlink_metadata(path).map_err(filesystem)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DirectQueueError::InvalidQueue);
    }
    path.canonicalize().map_err(filesystem)
}

fn bounded_error(error: &str) -> String {
    let mut value = error.trim().chars().take(4096).collect::<String>();
    if value.is_empty() {
        value = "direct worker failed without an error".into();
    }
    value
}

fn filesystem(error: std::io::Error) -> DirectQueueError {
    DirectQueueError::Filesystem(error.to_string())
}
fn serialization(error: serde_json::Error) -> DirectQueueError {
    DirectQueueError::Serialization(error.to_string())
}
