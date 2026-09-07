//! Strict operator command parsing and machine-readable output.

#[cfg(test)]
#[path = "cli_cycle_tests.rs"]
mod cycle_tests;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use pip_executor::{BoundedProcessRunner, sanitized_environment};
use pip_github::{
    GitHubAppCredentials, GitHubReader, GitHubWriter, UreqTransport, mint_installation_token,
};
use pip_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ReleaseError, ReleaseMetadata, create_release_manifest, sign_manifest, verify_release,
    verifying_key,
};

#[derive(Debug)]
pub enum CliError {
    Usage(&'static str),
    InvalidArgument(String),
    UnsafeInput(PathBuf),
    InputTooLarge(PathBuf),
    Filesystem(String),
    Ledger(String),
    Install(String),
    Reconciliation(String),
    Contract(String),
    Release(ReleaseError),
    Clock,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::InvalidArgument(argument) => write!(formatter, "invalid argument: {argument}"),
            Self::UnsafeInput(path) => write!(
                formatter,
                "input is not a regular non-symlink file: {}",
                path.display()
            ),
            Self::InputTooLarge(path) => write!(
                formatter,
                "input exceeded its size bound: {}",
                path.display()
            ),
            Self::Filesystem(error) => write!(formatter, "input filesystem error: {error}"),
            Self::Ledger(error) => write!(formatter, "ledger status failed: {error}"),
            Self::Install(error) => write!(formatter, "installation failed: {error}"),
            Self::Reconciliation(error) => {
                write!(formatter, "shadow reconciliation failed: {error}")
            }
            Self::Contract(error) => write!(formatter, "invalid worker result: {error}"),
            Self::Release(error) => error.fmt(formatter),
            Self::Clock => formatter.write_str("system clock is before the Unix epoch"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<ReleaseError> for CliError {
    fn from(error: ReleaseError) -> Self {
        Self::Release(error)
    }
}

pub fn run_cli(arguments: impl IntoIterator<Item = String>) -> Result<Value, CliError> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(CliError::Usage("a pip-control command is required"));
    };
    match command {
        "status" => status(&arguments[1..]),
        "validate-worker-result" => validate_worker_result(&arguments[1..]),
        "verify-release" => verify(&arguments[1..]),
        "seal-release" => seal(&arguments[1..]),
        "derive-public-key" => derive_public_key(&arguments[1..]),
        "shadow-reconcile" => shadow_reconcile(&arguments[1..]),
        "webhook-intake" => webhook_intake(&arguments[1..]),
        "webhook-spool-cycle" => webhook_spool_cycle(&arguments[1..]),
        "controller-cycle" => controller_cycle(&arguments[1..]),
        "direct-worker-cycle" => direct_worker_cycle(&arguments[1..]),
        "bootstrap-runtime" => bootstrap_runtime(&arguments[1..]),
        "scratch-retire" => scratch_retire(&arguments[1..]),
        "authorize-builder-retry" => authorize_retry(&arguments[1..], false),
        "authorize-review-retry" => authorize_retry(&arguments[1..], true),
        "authorize-publication-retry" => authorize_publication_retry(&arguments[1..]),
        "install-release" => install(&arguments[1..]),
        _ => Err(CliError::InvalidArgument(command.into())),
    }
}

fn validate_worker_result(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(arguments, &["--input"], &[])?;
    let bytes = read_bounded(Path::new(required(&options, "--input")?), 4 * 1024 * 1024)?;
    let value =
        serde_json::from_slice(&bytes).map_err(|error| CliError::Contract(error.to_string()))?;
    let result = pip_contracts::WorkerResult::decode(value)
        .map_err(|error| CliError::Contract(error.to_string()))?;
    result
        .validate()
        .map_err(|error| CliError::Contract(error.to_string()))?;
    Ok(
        json!({"ok":true,"role":result.common().role,"task_id":result.common().task_id,"workflow_authorized":false}),
    )
}

fn authorize_retry(arguments: &[String], review: bool) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--direct-queue",
            "--case",
            "--expected-revision",
            "--effect-id",
            "--expected-failures",
            "--request-id",
            "--reason",
        ],
        &[],
    )?;
    let (policy, mut store) = offline_recovery_store(&options)?;
    let number = |name: &'static str| -> Result<u64, CliError> {
        required(&options, name)?
            .parse()
            .map_err(|_| CliError::InvalidArgument(name.into()))
    };
    let request = crate::BuilderRetryRequest {
        case_key: required(&options, "--case")?.into(),
        expected_revision: number("--expected-revision")?,
        effect_id: required(&options, "--effect-id")?.into(),
        expected_failures: number("--expected-failures")?,
        request_id: required(&options, "--request-id")?.into(),
        reason: required(&options, "--reason")?.into(),
    };
    let authorize = if review {
        crate::authorize_review_retry
    } else {
        crate::authorize_builder_retry
    };
    let result = authorize(&mut store, &policy, &request, current_time()?, 0)
        .map_err(CliError::Reconciliation)?;
    Ok(
        json!({"ok":true,"result":format!("{result:?}"),"case_key":request.case_key,"request_id":request.request_id,
        "additional_attempts":1,"runtime_activated":false,"history_preserved":true}),
    )
}

fn authorize_publication_retry(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--direct-queue",
            "--case",
            "--expected-revision",
            "--expected-head",
            "--request-id",
            "--reason",
        ],
        &[],
    )?;
    let (policy, mut store) = offline_recovery_store(&options)?;
    let request = crate::PublicationRetryRequest {
        case_key: required(&options, "--case")?.into(),
        expected_revision: required(&options, "--expected-revision")?
            .parse()
            .map_err(|_| CliError::InvalidArgument("--expected-revision".into()))?,
        expected_head: required(&options, "--expected-head")?.into(),
        request_id: required(&options, "--request-id")?.into(),
        reason: required(&options, "--reason")?.into(),
    };
    let result =
        crate::authorize_publication_retry(&mut store, &policy, &request, current_time()?, 0)
            .map_err(CliError::Reconciliation)?;
    Ok(
        json!({"ok":true,"result":format!("{result:?}"),"case_key":request.case_key,"request_id":request.request_id,
        "additional_attempts":0,"runtime_activated":false,"history_preserved":true}),
    )
}

fn offline_recovery_store(
    options: &BTreeMap<String, String>,
) -> Result<(crate::RepositoryPolicy, Store), CliError> {
    let uid = rustix::process::geteuid().as_raw();
    if uid != 0 {
        return Err(CliError::Reconciliation(
            "work retry requires root authorization".into(),
        ));
    }
    let policy = crate::load_repository_policy(&read_bounded(
        Path::new(required(options, "--policy")?),
        1024 * 1024,
    )?)
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    if policy.intake.enabled || !policy.intake.paused || policy.dispatch_enabled {
        return Err(CliError::Reconciliation(
            "work retry requires an inert installed policy".into(),
        ));
    }
    crate::verify_scratch_runtime_stopped(&pip_hermes::ProcessRunner::default())
        .map_err(CliError::Reconciliation)?;
    let queue = Path::new(required(options, "--direct-queue")?);
    crate::DirectQueue::new(queue).map_err(|error| CliError::Reconciliation(error.to_string()))?;
    for directory in ["inbox", "results"] {
        if fs::read_dir(queue.join(directory))
            .map_err(|error| CliError::Filesystem(error.to_string()))?
            .next()
            .is_some()
        {
            return Err(CliError::Reconciliation(
                "direct queue must be drained before authorizing a retry".into(),
            ));
        }
    }
    let database = Path::new(required(options, "--database")?);
    let metadata =
        fs::symlink_metadata(database).map_err(|error| CliError::Filesystem(error.to_string()))?;
    if !metadata.is_file() {
        return Err(CliError::UnsafeInput(database.into()));
    }
    // Never create or migrate a database as a side effect of operator recovery.
    drop(Store::open_read_only(database).map_err(|error| CliError::Ledger(error.to_string()))?);
    let store = Store::open(database).map_err(|error| CliError::Ledger(error.to_string()))?;
    Ok((policy, store))
}

fn scratch_retire(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &["--policy", "--database", "--projection", "--now"],
        &[],
    )?;
    let policy = crate::load_repository_policy(&read_bounded(
        Path::new(required(&options, "--policy")?),
        1024 * 1024,
    )?)
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    if policy.intake.enabled || !policy.intake.paused || policy.dispatch_enabled {
        return Err(CliError::Reconciliation(
            "scratch retirement requires inert policy".into(),
        ));
    }
    crate::verify_scratch_runtime_stopped(&pip_hermes::ProcessRunner::default())
        .map_err(CliError::Reconciliation)?;
    let store = Store::open_read_only(required(&options, "--database")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    let projection = store
        .task_projection(required(&options, "--projection")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?
        .ok_or_else(|| CliError::Reconciliation("missing frozen task projection".into()))?;
    if projection.board != policy.board {
        return Err(CliError::Reconciliation("foreign task board".into()));
    }
    let body = &projection.desired["body"];
    let now = required(&options, "--now")?
        .parse::<u64>()
        .map_err(|_| CliError::InvalidArgument("--now".into()))?;
    crate::retire_hermes_scratch(&policy, &store, body, now, true)
        .map_err(CliError::Reconciliation)?;
    Ok(
        json!({"ok":true,"case_key":body["case_key"],"projection_key":body["projection_key"],"disposable_retired":true,"results_retained":true}),
    )
}

fn webhook_spool_cycle(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--github-token",
            "--webhook-secret",
            "--spool",
        ],
        &["--now", "--global-paused"],
    )?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let token = read_secret(Path::new(required(&options, "--github-token")?), 1024)?;
    let token = std::str::from_utf8(&token)
        .map_err(|_| CliError::InvalidArgument("--github-token".into()))?
        .trim();
    if token.is_empty() {
        return Err(CliError::InvalidArgument("--github-token".into()));
    }
    let secret = read_secret(Path::new(required(&options, "--webhook-secret")?), 1024)?;
    let secret = std::str::from_utf8(&secret)
        .map_err(|_| CliError::InvalidArgument("--webhook-secret".into()))?
        .trim()
        .as_bytes();
    if secret.is_empty() {
        return Err(CliError::InvalidArgument("--webhook-secret".into()));
    }
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    let global_paused = options
        .get("--global-paused")
        .map(|value| parse_bool(value, "--global-paused"))
        .transpose()?
        .unwrap_or(false);
    let reader = GitHubReader::new(
        UreqTransport::new(Duration::from_secs(20)),
        "https://api.github.com",
        token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let spool = crate::WebhookSpool::open(required(&options, "--spool")?)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let mut store = Store::open(required(&options, "--database")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    let report = crate::consume_webhook_spool_once(
        &reader,
        &policy,
        &mut store,
        &spool,
        secret,
        now,
        global_paused,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    serde_json::to_value(report).map_err(|error| CliError::Reconciliation(error.to_string()))
}

fn webhook_intake(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--github-token",
            "--webhook-secret",
            "--payload",
            "--delivery-id",
            "--event",
            "--signature",
        ],
        &["--now", "--global-paused"],
    )?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let token = read_secret(Path::new(required(&options, "--github-token")?), 1024)?;
    let token = std::str::from_utf8(&token)
        .map_err(|_| CliError::InvalidArgument("--github-token".into()))?
        .trim();
    if token.is_empty() {
        return Err(CliError::InvalidArgument("--github-token".into()));
    }
    let secret = read_secret(Path::new(required(&options, "--webhook-secret")?), 1024)?;
    let secret = std::str::from_utf8(&secret)
        .map_err(|_| CliError::InvalidArgument("--webhook-secret".into()))?
        .trim()
        .as_bytes();
    if secret.is_empty() {
        return Err(CliError::InvalidArgument("--webhook-secret".into()));
    }
    let payload = read_bounded(Path::new(required(&options, "--payload")?), 4 * 1024 * 1024)?;
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    let global_paused = options
        .get("--global-paused")
        .map(|value| parse_bool(value, "--global-paused"))
        .transpose()?
        .unwrap_or(false);
    let reader = GitHubReader::new(
        UreqTransport::new(Duration::from_secs(20)),
        "https://api.github.com",
        token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let mut store = Store::open(required(&options, "--database")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    let report = crate::ingest_webhook(
        &reader,
        &policy,
        &mut store,
        crate::WebhookEnvelope {
            delivery_id: required(&options, "--delivery-id")?,
            event_name: required(&options, "--event")?,
            signature: required(&options, "--signature")?,
            payload: &payload,
            received_at: now,
        },
        secret,
        now,
        global_paused,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    serde_json::to_value(report).map_err(|error| CliError::Reconciliation(error.to_string()))
}

pub fn run_git_askpass(arguments: impl IntoIterator<Item = String>) -> Result<String, CliError> {
    if std::env::var("PIP_GIT_ASKPASS").as_deref() != Ok("1") {
        return Err(CliError::InvalidArgument("askpass mode".into()));
    }
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let [prompt] = arguments.as_slice() else {
        return Err(CliError::InvalidArgument("askpass prompt".into()));
    };
    let credential_path = std::env::var_os("PIP_GIT_TOKEN_FILE")
        .ok_or_else(|| CliError::InvalidArgument("askpass credential".into()))?;
    let credential = read_secret(Path::new(&credential_path), 1024)?;
    let credential = std::str::from_utf8(&credential)
        .map_err(|_| CliError::InvalidArgument("askpass credential".into()))?
        .trim();
    if credential.is_empty() || credential.chars().any(char::is_control) {
        return Err(CliError::InvalidArgument("askpass credential".into()));
    }
    match prompt.trim_end() {
        "Username for 'https://github.com':" => Ok("x-access-token".into()),
        "Password for 'https://x-access-token@github.com':" => Ok(credential.into()),
        _ => Err(CliError::InvalidArgument("askpass prompt".into())),
    }
}

fn bootstrap_runtime(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--hermes-root",
            "--skills-root",
            "--auth-source",
            "--hermes",
        ],
        &[],
    )?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let outcome = crate::bootstrap_hermes_runtime_with(
        &policy,
        Path::new(required(&options, "--hermes-root")?),
        Path::new(required(&options, "--skills-root")?),
        Path::new(required(&options, "--auth-source")?),
        required(&options, "--hermes")?,
        pip_hermes::ProcessRunner::for_hermes_root(Path::new(required(&options, "--hermes-root")?))
            .map_err(|error| CliError::Reconciliation(error.to_string()))?,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    Ok(json!({
        "ok": true,
        "repository": policy.repository.full_name(),
        "policy_revision": policy.revision,
        "runtime": outcome,
    }))
}

fn controller_cycle(arguments: &[String]) -> Result<Value, CliError> {
    controller_cycle_with_transport(arguments, UreqTransport::new(Duration::from_secs(20)))
}

fn controller_cycle_with_transport<T>(arguments: &[String], transport: T) -> Result<Value, CliError>
where
    T: pip_github::ReadTransport + pip_github::MutationTransport + Clone,
{
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--github-token",
            "--github-reviewer-general-app",
            "--github-reviewer-general-key",
            "--github-reviewer-secperf-app",
            "--github-reviewer-secperf-key",
            "--git-askpass",
            "--hermes",
            "--owner",
            "--skills-commit-file",
            "--direct-queue",
        ],
        &[
            "--now",
            "--lease-seconds",
            "--global-paused",
            "--commit-signing-identity",
            "--commit-signing-key",
        ],
    )?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let global_paused = options
        .get("--global-paused")
        .map(|value| parse_bool(value, "--global-paused"))
        .transpose()?
        .unwrap_or(false);
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    // Collection always precedes external dependencies, even when active:
    // a GitHub outage must not strand a finished model result. This phase
    // neither advances cases nor dispatches. A never-bootstrapped inert
    // installation still creates no database.
    let database = Path::new(required(&options, "--database")?);
    let (direct_worker, worker_result) = if database
        .try_exists()
        .map_err(|error| CliError::Filesystem(error.to_string()))?
    {
        let mut store =
            Store::open(database).map_err(|error| CliError::Ledger(error.to_string()))?;
        let direct_worker = cycle_observation((|| {
            let queue = crate::DirectQueue::new(required(&options, "--direct-queue")?)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?;
            crate::reconcile_direct_queue_once(
                &mut store,
                &policy,
                &queue,
                required(&options, "--owner")?,
                now,
                1,
                false,
            )
            .map_err(|error| CliError::Reconciliation(error.to_string()))
        })());
        let worker_result = cycle_observation(crate::reconcile_completed_once_with(
            &mut store,
            &policy,
            pip_hermes::ProcessRunner::default(),
            required(&options, "--hermes")?,
            now,
            false,
        ));
        (direct_worker, worker_result)
    } else {
        let absent = json!({"result":"not_initialized"});
        (absent.clone(), absent)
    };
    if global_paused || policy.intake.paused || !policy.dispatch_enabled {
        return Ok(json!({
            "ok": direct_worker["result"] != "error" && worker_result["result"] != "error",
            "result": "disabled",
            "repository": policy.repository.full_name(),
            "policy_revision": policy.revision,
            "direct_worker": direct_worker,
            "worker_result": worker_result,
        }));
    }
    let collection = json!({"direct_worker":direct_worker,"worker_result":worker_result});
    let token = read_secret(Path::new(required(&options, "--github-token")?), 1024)?;
    let token = std::str::from_utf8(&token)
        .map_err(|_| CliError::InvalidArgument("--github-token".into()))?
        .trim();
    if token.is_empty() {
        return Err(CliError::InvalidArgument("--github-token".into()));
    }
    let skills_commit = read_bounded(Path::new(required(&options, "--skills-commit-file")?), 128)?;
    let skills_commit = std::str::from_utf8(&skills_commit)
        .map_err(|_| CliError::InvalidArgument("--skills-commit-file".into()))?
        .trim();
    if skills_commit.len() != 40
        || !skills_commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CliError::InvalidArgument("--skills-commit-file".into()));
    }
    let lease_seconds = options
        .get("--lease-seconds")
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| CliError::InvalidArgument("--lease-seconds".into()))
        })
        .transpose()?
        .unwrap_or(60);
    let reader = GitHubReader::new(
        transport.clone(),
        "https://api.github.com",
        token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let writer = GitHubWriter::new(
        transport.clone(),
        "https://api.github.com",
        token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let general_review_writer = AppReviewWriter {
        options: &options,
        repository_id: policy.repository.id,
        general: true,
        now,
    };
    let secperf_review_writer = AppReviewWriter {
        options: &options,
        repository_id: policy.repository.id,
        general: false,
        now,
    };
    let mut store = Store::open(required(&options, "--database")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    let workspace_lifecycle = crate::reconcile_workspace_lifecycle_once(&mut store, &policy, now);
    let workspace_ready = workspace_lifecycle.as_ref().is_ok_and(|state| state.ready);
    let workspace_lifecycle = cycle_observation(workspace_lifecycle);
    let intake = if policy.intake.enabled && workspace_ready {
        cycle_observation(crate::reconcile_intake(
            &reader, &policy, &mut store, now, false,
        ))
    } else {
        json!({"result": "disabled"})
    };
    // Select the case before its checks and effects. A failed capability on one
    // case is a report for that case, never a repository-wide authorization veto.
    let mut cases = store
        .status(now)
        .map_err(|error| CliError::Ledger(error.to_string()))?
        .cases
        .into_iter()
        .filter(|case| case.repository_id == policy.repository.id)
        .collect::<Vec<_>>();
    cases.sort_by(|left, right| left.case_key.cmp(&right.case_key));
    let mut case_reports = Vec::with_capacity(cases.len());
    let mut work_cases = Vec::new();
    for case in cases {
        let scope = crate::RepositoryScope::case(&policy, &case.case_key);
        let takeover = crate::reconcile_takeover_once(&reader, scope, &mut store, now);
        let authorization = crate::reconcile_active_authorization(&reader, scope, &mut store, now);
        let bounds = crate::enforce_operational_bounds(&mut store, scope, now);
        // Errors are observations, never authorization. Keep collection and unrelated
        // capabilities available, but fail closed when a prerequisite cannot be checked.
        let advancement_authorized = authorization
            .as_ref()
            .is_ok_and(|state| state.is_authorized())
            && takeover.is_ok()
            && bounds.is_ok();
        let takeover = cycle_observation(takeover);
        let authorization_has_errors = authorization.as_ref().is_ok_and(|state| state.has_errors());
        let authorization = cycle_observation(authorization);
        let bounds = cycle_observation(bounds);
        let work_authorized = advancement_authorized && workspace_ready;
        if work_authorized {
            work_cases.push(case.case_key.clone());
        }
        let (result, ci) = if advancement_authorized {
            (
                cycle_observation(crate::ingest_completed_once(
                    &mut store,
                    scope,
                    required(&options, "--hermes")?,
                    now,
                )),
                cycle_observation(crate::reconcile_ci_once(&reader, scope, &mut store, now)),
            )
        } else {
            let blocked = json!({"result": "authorization_blocked"});
            (blocked.clone(), blocked)
        };
        let direct_worker = cycle_observation((|| {
            let direct_queue = crate::DirectQueue::new(required(&options, "--direct-queue")?)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?;
            crate::collect_direct_queue_once(&mut store, scope, &direct_queue, now, work_authorized)
                .map_err(|error| CliError::Reconciliation(error.to_string()))
        })());
        let draft_pull_request = cycle_observation(crate::publish_draft_pull_request_once(
            &writer,
            scope,
            &mut store,
            Path::new(required(&options, "--git-askpass")?),
            Path::new(required(&options, "--github-token")?),
            options
                .get("--commit-signing-identity")
                .zip(options.get("--commit-signing-key"))
                .map(|(identity, key)| crate::CommitSigningCredentials {
                    identity_file: Path::new(identity),
                    key_file: Path::new(key),
                }),
            now,
            required(&options, "--owner")?,
            lease_seconds,
            work_authorized,
        ));
        let plan_publication = cycle_observation(crate::publish_plan_once(
            &writer,
            scope,
            &mut store,
            now,
            required(&options, "--owner")?,
            lease_seconds,
            advancement_authorized,
        ));
        let review_publication = cycle_observation(crate::publish_reviews_once(
            &general_review_writer,
            &secperf_review_writer,
            scope,
            &mut store,
            now,
            required(&options, "--owner")?,
            lease_seconds,
            advancement_authorized,
        ));
        let final_preflight = cycle_observation(crate::reconcile_final_preflight_once(
            &reader,
            scope,
            &mut store,
            now,
            required(&options, "--owner")?,
            lease_seconds,
            advancement_authorized,
        ));
        let disposition = cycle_observation(crate::consume_disposition_once(
            (&reader, &writer),
            scope,
            &mut store,
            now,
            required(&options, "--owner")?,
            lease_seconds,
            advancement_authorized,
        ));
        let dispatch = cycle_observation(crate::dispatch_once(
            &mut store,
            scope,
            crate::DispatchCycleContext {
                skills_repository_commit: skills_commit,
                hermes_program: required(&options, "--hermes")?,
                owner: required(&options, "--owner")?,
                now,
                lease_seconds,
                authorization_valid: work_authorized,
            },
        ));
        let mut case_report = json!({
            "case_key": case.case_key,
            "worker_result": result,
            "direct_worker": direct_worker,
            "ci": ci,
            "takeover": takeover,
            "authorization": authorization,
            "operational_bounds": bounds,
            "draft_pull_request": draft_pull_request,
            "plan_publication": plan_publication,
            "review_publication": review_publication,
            "final_preflight": final_preflight,
            "disposition": disposition,
            "dispatch": dispatch,
        });
        case_report["ok"] = json!(
            !authorization_has_errors
                && case_report
                    .as_object()
                    .expect("case report")
                    .values()
                    .all(|value| value["result"] != "error")
        );
        case_reports.push(case_report);
    }
    let direct_dispatch = (|| {
        let queue = crate::DirectQueue::new(required(&options, "--direct-queue")?)
            .map_err(|error| CliError::Reconciliation(error.to_string()))?;
        crate::schedule_direct_queue_once(
            &mut store,
            &policy,
            &work_cases,
            &queue,
            required(&options, "--owner")?,
            now,
            crate::recommended_direct_lease_seconds(&policy)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?,
        )
        .map_err(|error| CliError::Reconciliation(error.to_string()))
    })();
    let scheduling_ok = direct_dispatch
        .as_ref()
        .is_ok_and(|report| report.errors.is_empty());
    let direct_dispatch = cycle_observation(direct_dispatch);
    let ok = collection["direct_worker"]["result"] != "error"
        && collection["worker_result"]["result"] != "error"
        && workspace_lifecycle["result"] != "error"
        && intake["result"] != "error"
        && case_reports.iter().all(|case| case["ok"] == true)
        && scheduling_ok;
    Ok(json!({
        "report_format": 2, "result": "active", "ok": ok,
        "observed_at": now, "repository": policy.repository.full_name(),
        "policy_revision": policy.revision, "collection": collection,
        "workspace_lifecycle": workspace_lifecycle, "intake": intake,
        "cases": case_reports, "direct_dispatch":direct_dispatch, "merge": {"result":"human_only"}
    }))
}

fn cycle_observation<T: serde::Serialize, E: fmt::Display>(result: Result<T, E>) -> Value {
    match result {
        Ok(value) => serde_json::to_value(value)
            .unwrap_or_else(|error| json!({"result":"error", "error":error.to_string()})),
        Err(error) => json!({"result":"error", "error":error.to_string()}),
    }
}

struct AppReviewWriter<'a> {
    options: &'a BTreeMap<String, String>,
    repository_id: u64,
    general: bool,
    now: u64,
}

impl AppReviewWriter<'_> {
    fn writer(&self) -> Result<GitHubWriter<UreqTransport>, CliError> {
        let general = read_app_identity(
            Path::new(required(self.options, "--github-reviewer-general-app")?),
            self.repository_id,
            "--github-reviewer-general-app",
        )?;
        let secperf = read_app_identity(
            Path::new(required(self.options, "--github-reviewer-secperf-app")?),
            self.repository_id,
            "--github-reviewer-secperf-app",
        )?;
        if general.app_id == secperf.app_id || general.installation_id == secperf.installation_id {
            return Err(CliError::InvalidArgument("--github-reviewer-apps".into()));
        }
        let (app, key_option) = if self.general {
            (general, "--github-reviewer-general-key")
        } else {
            (secperf, "--github-reviewer-secperf-key")
        };
        let mut key = read_secret(Path::new(required(self.options, key_option)?), 32 * 1024)?;
        let transport = UreqTransport::new(Duration::from_secs(20));
        let token = mint_installation_token(
            &transport,
            "https://api.github.com",
            &app.credentials(&key),
            self.now,
        );
        key.fill(0);
        let token = token
            .map_err(|error| CliError::Reconciliation(error.to_string()))?
            .token;
        GitHubWriter::new(
            transport,
            "https://api.github.com",
            &token,
            4 * 1024 * 1024,
            10,
        )
        .map_err(|error| CliError::Reconciliation(error.to_string()))
    }
}

impl crate::ReviewWriter for AppReviewWriter<'_> {
    fn ensure_review(
        &self,
        spec: &pip_github::ReviewMutationSpec,
    ) -> Result<pip_github::MutationResult, pip_github::GitHubError> {
        // Construction is deliberately credential-free. Only an actual review
        // publication acquires its lane's short-lived installation token.
        self.writer()
            .map_err(|error| pip_github::GitHubError::Transport(error.to_string()))?
            .ensure_pull_request_review(spec)
    }
}

#[cfg(test)]
mod review_credentials_tests {
    use super::*;

    #[test]
    fn missing_review_key_remains_a_publication_error() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = BTreeMap::new();
        for (lane, app_id) in [("general", 123), ("secperf", 124)] {
            let app = directory.path().join(format!("{lane}.json"));
            fs::write(
                &app,
                serde_json::to_vec(&json!({
                    "app_id": app_id, "installation_id": app_id + 100, "repository_id": 789
                }))
                .unwrap(),
            )
            .unwrap();
            options.insert(
                format!("--github-reviewer-{lane}-app"),
                app.to_str().unwrap().into(),
            );
            options.insert(
                format!("--github-reviewer-{lane}-key"),
                directory
                    .path()
                    .join("missing.pem")
                    .to_str()
                    .unwrap()
                    .into(),
            );
        }
        for general in [true, false] {
            let writer = AppReviewWriter {
                options: &options,
                repository_id: 789,
                general,
                now: 100,
            };
            assert!(matches!(writer.writer(), Err(CliError::Filesystem(_))));
        }
    }

    #[test]
    fn duplicate_review_apps_are_rejected_before_key_read_or_network() {
        let directory = tempfile::tempdir().unwrap();
        let app = directory.path().join("app.json");
        fs::write(
            &app,
            br#"{"app_id":123,"installation_id":456,"repository_id":789}"#,
        )
        .unwrap();
        let options = BTreeMap::from([
            (
                "--github-reviewer-general-app".into(),
                app.to_str().unwrap().into(),
            ),
            (
                "--github-reviewer-secperf-app".into(),
                app.to_str().unwrap().into(),
            ),
        ]);
        let writer = AppReviewWriter {
            options: &options,
            repository_id: 789,
            general: true,
            now: 100,
        };
        assert!(
            matches!(writer.writer(), Err(CliError::InvalidArgument(argument))
            if argument == "--github-reviewer-apps")
        );
    }

    #[test]
    fn idle_review_publication_does_not_require_app_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::open(directory.path().join("ledger.db")).unwrap();
        let policy = crate::load_repository_policy(include_bytes!(
            "../../../config/target/repositories/mdk.json"
        ))
        .unwrap();
        let options = BTreeMap::new();
        let writer = AppReviewWriter {
            options: &options,
            repository_id: policy.repository.id,
            general: true,
            now: 100,
        };
        assert_eq!(
            crate::publish_reviews_once(
                &writer,
                &writer,
                &policy,
                &mut store,
                100,
                "controller",
                30,
                true,
            )
            .unwrap(),
            crate::ReviewPublicationCycle::Idle
        );
        // Missing secrets have not been read, but must fail when actually needed.
        assert!(writer.writer().is_err());
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitHubAppIdentity {
    app_id: u64,
    installation_id: u64,
    repository_id: u64,
}

impl GitHubAppIdentity {
    fn credentials<'a>(&self, private_key_pem: &'a [u8]) -> GitHubAppCredentials<'a> {
        GitHubAppCredentials {
            app_id: self.app_id,
            installation_id: self.installation_id,
            repository_id: self.repository_id,
            private_key_pem,
        }
    }
}

fn read_app_identity(
    path: &Path,
    expected_repository_id: u64,
    argument: &str,
) -> Result<GitHubAppIdentity, CliError> {
    let bytes = read_bounded(path, 4096)?;
    let identity: GitHubAppIdentity =
        serde_json::from_slice(&bytes).map_err(|_| CliError::InvalidArgument(argument.into()))?;
    if identity.app_id == 0
        || identity.installation_id == 0
        || identity.repository_id != expected_repository_id
    {
        return Err(CliError::InvalidArgument(argument.into()));
    }
    Ok(identity)
}

fn direct_worker_cycle(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--direct-queue",
            "--cursor",
            "--git",
            "--skills-root",
        ],
        &["--now"],
    )?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    if policy.intake.paused || !policy.dispatch_enabled {
        return Ok(json!({
            "ok": true,
            "result": "disabled",
            "repository": policy.repository.full_name(),
            "policy_revision": policy.revision,
        }));
    }
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    crate::workspace_lifecycle::ensure_direct_workspace_capacity(&policy)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let runtime = crate::CursorDirectRuntime::new(
        BoundedProcessRunner,
        required(&options, "--cursor")?,
        required(&options, "--git")?,
        &policy.workspace,
        &policy.artifacts,
        required(&options, "--skills-root")?,
        sanitized_environment(),
        4 * 1024 * 1024,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let queue = crate::DirectQueue::new(required(&options, "--direct-queue")?)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let result = crate::execute_direct_queue_once(&runtime, &queue, now)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    Ok(json!({
        "ok": true,
        "repository": policy.repository.full_name(),
        "policy_revision": policy.revision,
        "direct_worker": result,
    }))
}

fn parse_bool(value: &str, name: &str) -> Result<bool, CliError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(CliError::InvalidArgument(name.into())),
    }
}

fn derive_public_key(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(arguments, &["--signing-key"], &[])?;
    let signing_key = read_secret(Path::new(required(&options, "--signing-key")?), 1024)?;
    let signing_key = std::str::from_utf8(&signing_key)
        .map_err(|_| CliError::InvalidArgument("--signing-key".into()))?;
    Ok(json!({
        "ok": true,
        "public_key": verifying_key(signing_key.trim())?,
    }))
}

fn install(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--cohort",
            "--public-key",
            "--install-root",
            "--config-root",
            "--unit-root",
            "--state-root",
            "--manifest-sha256",
            "--binary-sha256",
            "--systemctl",
            "--state-uid",
            "--state-gid",
        ],
        &[],
    )?;
    let public_key = read_bounded(Path::new(required(&options, "--public-key")?), 1024)?;
    let public_key = std::str::from_utf8(&public_key)
        .map_err(|_| CliError::InvalidArgument("--public-key".into()))?
        .trim();
    let state_uid = required(&options, "--state-uid")?
        .parse::<u32>()
        .map_err(|_| CliError::InvalidArgument("--state-uid".into()))?;
    let state_gid = required(&options, "--state-gid")?
        .parse::<u32>()
        .map_err(|_| CliError::InvalidArgument("--state-gid".into()))?;
    let manifest_sha256 = sha256_option(&options, "--manifest-sha256")?;
    let binary_sha256 = sha256_option(&options, "--binary-sha256")?;
    let outcome = crate::install_host_release(
        required(&options, "--cohort")?,
        public_key,
        &crate::InstallLayout {
            install_root: required(&options, "--install-root")?.into(),
            config_root: required(&options, "--config-root")?.into(),
            unit_root: required(&options, "--unit-root")?.into(),
            state_root: required(&options, "--state-root")?.into(),
        },
        manifest_sha256,
        binary_sha256,
        &crate::HostInstallOptions {
            systemctl: required(&options, "--systemctl")?.into(),
            state_uid,
            state_gid,
        },
    )
    .map_err(|error| CliError::Install(error.to_string()))?;
    Ok(json!({
        "ok": true,
        "result": match outcome.result {
            crate::InstallResult::Installed => "installed",
            crate::InstallResult::Existing => "existing",
        },
        "release_id": outcome.release_id,
        "source_commit": outcome.source_commit,
    }))
}

fn sha256_option<'a>(
    options: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, CliError> {
    let value = required(options, name)?;
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value)
    } else {
        Err(CliError::InvalidArgument(name.into()))
    }
}

fn shadow_reconcile(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(arguments, &["--policy", "--github-token"], &["--now"])?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let token = read_secret(Path::new(required(&options, "--github-token")?), 1024)?;
    let token = std::str::from_utf8(&token)
        .map_err(|_| CliError::InvalidArgument("--github-token".into()))?
        .trim();
    if token.is_empty() {
        return Err(CliError::InvalidArgument("--github-token".into()));
    }
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    let reader = GitHubReader::new(
        UreqTransport::new(Duration::from_secs(20)),
        "https://api.github.com",
        token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let report = crate::reconcile_read_only(&reader, &policy, now, 0, 0)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    serde_json::to_value(report).map_err(|error| CliError::Reconciliation(error.to_string()))
}

fn seal(arguments: &[String]) -> Result<Value, CliError> {
    let required_options = [
        "--release-root",
        "--version",
        "--source-commit",
        "--cargo-lock-sha256",
        "--target",
        "--rust-toolchain",
        "--built-at",
        "--builder-identity",
        "--signing-key",
        "--expected-public-key",
        "--manifest-output",
        "--signature-output",
    ];
    let options = options(arguments, &required_options, &[])?;
    let signing_key_path = Path::new(required(&options, "--signing-key")?);
    let signing_key = read_secret(signing_key_path, 1024)?;
    let signing_key = std::str::from_utf8(&signing_key)
        .map_err(|_| CliError::InvalidArgument("--signing-key".into()))?;
    let expected_public_key = read_bounded(
        Path::new(required(&options, "--expected-public-key")?),
        1024,
    )?;
    let expected_public_key = std::str::from_utf8(&expected_public_key)
        .map_err(|_| CliError::InvalidArgument("--expected-public-key".into()))?
        .trim();
    if verifying_key(signing_key)? != expected_public_key {
        return Err(CliError::InvalidArgument("--signing-key".into()));
    }
    let manifest = create_release_manifest(
        required(&options, "--release-root")?,
        &ReleaseMetadata {
            version: required(&options, "--version")?.into(),
            source_commit: required(&options, "--source-commit")?.into(),
            cargo_lock_sha256: required(&options, "--cargo-lock-sha256")?.into(),
            target: required(&options, "--target")?.into(),
            rust_toolchain: required(&options, "--rust-toolchain")?.into(),
            built_at: required(&options, "--built-at")?.into(),
            builder_identity: required(&options, "--builder-identity")?.into(),
            workflow_version: 3,
            contract_version: 2,
        },
    )?;
    let manifest_bytes =
        serde_json::to_vec(&manifest).map_err(|error| CliError::Filesystem(error.to_string()))?;
    let signature = sign_manifest(&manifest_bytes, signing_key)?;
    let manifest_output = Path::new(required(&options, "--manifest-output")?);
    let signature_output = Path::new(required(&options, "--signature-output")?);
    write_new(manifest_output, &manifest_bytes, 0o444)?;
    if let Err(error) = write_new(signature_output, signature.as_bytes(), 0o444) {
        let _ = fs::remove_file(manifest_output);
        return Err(error);
    }
    Ok(json!({
        "ok": true,
        "source_commit": manifest.source_commit,
        "artifact_count": manifest.artifacts.len(),
        "manifest_output": manifest_output,
        "signature_output": signature_output,
    }))
}

fn status(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &["--database"],
        &["--now", "--case", "--attempt"],
    )?;
    if options.contains_key("--case") && options.contains_key("--attempt") {
        return Err(CliError::Usage("select either --case or --attempt"));
    }
    let database = required(&options, "--database")?;
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
    let store =
        Store::open_read_only(database).map_err(|error| CliError::Ledger(error.to_string()))?;
    if let Some(case_key) = options.get("--case") {
        let case = store
            .case(case_key)
            .map_err(|error| CliError::Ledger(error.to_string()))?
            .ok_or_else(|| CliError::InvalidArgument("unknown --case".into()))?;
        let history = store
            .immutable_history_for_case(case_key)
            .map_err(|error| CliError::Ledger(error.to_string()))?;
        return Ok(json!({"ok": true, "observed_at": now, "case": case, "history": history}));
    }
    if let Some(attempt_id) = options.get("--attempt") {
        let attempt_id = attempt_id
            .parse::<u64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| CliError::InvalidArgument("--attempt".into()))?;
        let attempt = store
            .direct_attempt(attempt_id)
            .map_err(|error| CliError::Ledger(error.to_string()))?
            .ok_or_else(|| CliError::InvalidArgument("unknown --attempt".into()))?;
        return Ok(json!({"ok": true, "observed_at": now, "attempt": attempt}));
    }
    let ledger = store
        .status(now)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    Ok(json!({
        "ok": true,
        "observed_at": now,
        "ledger": ledger,
    }))
}

fn verify(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--release-root",
            "--manifest",
            "--signature",
            "--public-key",
        ],
        &[],
    )?;
    let release_root = required(&options, "--release-root")?;
    let manifest = read_bounded(Path::new(required(&options, "--manifest")?), 1024 * 1024)?;
    let signature = read_bounded(Path::new(required(&options, "--signature")?), 1024)?;
    let public_key = read_bounded(Path::new(required(&options, "--public-key")?), 1024)?;
    let signature = std::str::from_utf8(&signature)
        .map_err(|_| CliError::InvalidArgument("--signature".into()))?;
    let public_key = std::str::from_utf8(&public_key)
        .map_err(|_| CliError::InvalidArgument("--public-key".into()))?;
    let verified = verify_release(release_root, &manifest, signature, public_key)?;
    Ok(json!({
        "ok": true,
        "source_commit": verified.source_commit(),
        "binary_path": verified.binary_path(),
        "binary_sha256": verified.binary_sha256(),
        "manifest_sha256": verified.manifest_sha256(),
        "artifact_count": verified.artifact_count(),
    }))
}

fn options(
    arguments: &[String],
    required_names: &[&str],
    optional_names: &[&str],
) -> Result<BTreeMap<String, String>, CliError> {
    if !arguments.len().is_multiple_of(2) {
        return Err(CliError::Usage("command options must be name/value pairs"));
    }
    let allowed = required_names
        .iter()
        .chain(optional_names)
        .copied()
        .collect::<Vec<_>>();
    let mut result = BTreeMap::new();
    for pair in arguments.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str()) || pair[1].is_empty() {
            return Err(CliError::InvalidArgument(pair[0].clone()));
        }
        if result.insert(pair[0].clone(), pair[1].clone()).is_some() {
            return Err(CliError::InvalidArgument(pair[0].clone()));
        }
    }
    if required_names
        .iter()
        .any(|name| !result.contains_key(*name))
    {
        return Err(CliError::Usage("one or more required options are missing"));
    }
    Ok(result)
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, CliError> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or(CliError::Usage("required option is missing"))
}

fn current_time() -> Result<u64, CliError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| CliError::Clock)
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, CliError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| CliError::Filesystem(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::UnsafeInput(path.to_owned()));
    }
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(CliError::InputTooLarge(path.to_owned()));
    }
    let mut file = File::open(path).map_err(|error| CliError::Filesystem(error.to_string()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(max_bytes));
    Read::by_ref(&mut file)
        .take(u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::Filesystem(error.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(CliError::InputTooLarge(path.to_owned()));
    }
    Ok(bytes)
}

fn read_secret(path: &Path, max_bytes: usize) -> Result<Vec<u8>, CliError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| CliError::Filesystem(error.to_string()))?;
    if !crate::secret_file::metadata_is_safe_secret_file(&metadata) {
        return Err(CliError::UnsafeInput(path.to_owned()));
    }
    read_bounded(path, max_bytes)
}

fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<(), CliError> {
    if path.file_name().is_none() {
        return Err(CliError::UnsafeInput(path.to_owned()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| CliError::UnsafeInput(path.to_owned()))?;
    let metadata =
        fs::symlink_metadata(parent).map_err(|error| CliError::Filesystem(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CliError::UnsafeInput(path.to_owned()));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| CliError::Filesystem(error.to_string()))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(CliError::Filesystem(error.to_string()));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| CliError::Filesystem(error.to_string()))
}
