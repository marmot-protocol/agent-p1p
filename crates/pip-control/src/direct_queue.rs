//! Filesystem bridge between the authoritative controller and an unprivileged direct worker.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use pip_contracts::WorkerResult;
use pip_controller::{DirectTaskSpec, IngestResult, ingest_worker_result};
use pip_store::{ClaimedEffect, DirectAttemptStatus, Store, StoreError};
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
            Self::Complete { attempt_id, .. } | Self::Failed { attempt_id, .. } => *attempt_id,
        }
    }

    const fn schema_version(&self) -> u32 {
        match self {
            Self::Complete { schema_version, .. } | Self::Failed { schema_version, .. } => {
                *schema_version
            }
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
    if !authorization_valid {
        return Ok(DirectQueueCycle::AuthorizationBlocked);
    }
    if let Some(path) = queue_files(&queue.results)?.into_iter().next() {
        return ingest_result(store, policy, queue, &path, now);
    }
    let Some(claimed) =
        store.claim_effect_matching(owner, now, lease_seconds, &["RUN_DIRECT_WORKER"])?
    else {
        return Ok(DirectQueueCycle::Idle);
    };
    let task: DirectTaskSpec = serde_json::from_value(claimed.payload.clone())
        .map_err(|_| DirectQueueError::InvalidEnvelope)?;
    let case = store
        .case(&claimed.case_key)?
        .ok_or(DirectQueueError::InvalidEnvelope)?;
    validate_job(&claimed, &case, &task, policy).map_err(|_| DirectQueueError::InvalidEnvelope)?;
    let attempt_id = store.begin_direct_attempt(&claimed, &task.task_id, now)?;
    let envelope = WorkEnvelope {
        schema_version: 1,
        attempt_id,
        claimed: claimed.clone(),
        task: task.clone(),
    };
    if let Err(error) = write_new(&queue.input(attempt_id), &envelope, 0o440) {
        store.fail_direct_attempt(attempt_id, &claimed.lease_owner, now, &error.to_string())?;
        store.release_effect(&claimed.effect_id, &claimed.lease_owner)?;
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
            ResultEnvelope::Failed {
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
    if store.run_by_task_id(&attempt.task_id)?.is_some() {
        queue.archive(attempt_id)?;
        return Ok(DirectQueueCycle::Cleaned { attempt_id });
    }
    let case = store
        .case(&attempt.case_key)?
        .ok_or(DirectQueueError::InvalidEnvelope)?;
    let binding = validate_job(&work.claimed, &case, &work.task, policy)
        .map_err(|_| DirectQueueError::InvalidEnvelope)?;
    match envelope {
        ResultEnvelope::Failed { error, .. } => {
            if attempt.status == DirectAttemptStatus::Running {
                store.fail_direct_attempt(
                    attempt_id,
                    &attempt.lease_owner,
                    now,
                    &bounded_error(&error),
                )?;
                store.release_effect(&attempt.effect_id, &attempt.lease_owner)?;
            }
            queue.archive(attempt_id)?;
            Ok(DirectQueueCycle::Failed { attempt_id })
        }
        ResultEnvelope::Complete { result, .. } => {
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
            let ingested = ingest_worker_result(store, &policy.case_policy(), &binding, &result)
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
