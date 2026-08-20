//! Strict operator command parsing and machine-readable output.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use pip_executor::{BoundedProcessRunner, sanitized_environment};
use pip_github::{GitHubReader, GitHubWriter, UreqTransport};
use pip_store::Store;
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
        "verify-release" => verify(&arguments[1..]),
        "seal-release" => seal(&arguments[1..]),
        "derive-public-key" => derive_public_key(&arguments[1..]),
        "shadow-reconcile" => shadow_reconcile(&arguments[1..]),
        "controller-cycle" => controller_cycle(&arguments[1..]),
        "direct-worker-cycle" => direct_worker_cycle(&arguments[1..]),
        "bootstrap-runtime" => bootstrap_runtime(&arguments[1..]),
        "install-release" => install(&arguments[1..]),
        _ => Err(CliError::InvalidArgument(command.into())),
    }
}

pub fn run_git_askpass(arguments: impl IntoIterator<Item = String>) -> Result<String, CliError> {
    if std::env::var("PIP_V2_GIT_ASKPASS").as_deref() != Ok("1") {
        return Err(CliError::InvalidArgument("askpass mode".into()));
    }
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let [prompt] = arguments.as_slice() else {
        return Err(CliError::InvalidArgument("askpass prompt".into()));
    };
    let credential_path = std::env::var_os("PIP_V2_GIT_TOKEN_FILE")
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
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--github-token",
            "--github-reviewer-general-token",
            "--github-reviewer-secperf-token",
            "--git-askpass",
            "--hermes",
            "--owner",
            "--skills-commit-file",
        ],
        &["--now", "--lease-seconds", "--global-paused"],
    )?;
    let policy_bytes = read_bounded(Path::new(required(&options, "--policy")?), 1024 * 1024)?;
    let policy = crate::load_repository_policy(&policy_bytes)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let global_paused = options
        .get("--global-paused")
        .map(|value| parse_bool(value, "--global-paused"))
        .transpose()?
        .unwrap_or(false);
    if global_paused || policy.intake.paused || !policy.dispatch_enabled {
        return Ok(json!({
            "ok": true,
            "result": "disabled",
            "repository": policy.repository.full_name(),
            "policy_revision": policy.revision,
        }));
    }
    let token = read_secret(Path::new(required(&options, "--github-token")?), 1024)?;
    let token = std::str::from_utf8(&token)
        .map_err(|_| CliError::InvalidArgument("--github-token".into()))?
        .trim();
    if token.is_empty() {
        return Err(CliError::InvalidArgument("--github-token".into()));
    }
    let general_token = read_secret(
        Path::new(required(&options, "--github-reviewer-general-token")?),
        1024,
    )?;
    let general_token = std::str::from_utf8(&general_token)
        .map_err(|_| CliError::InvalidArgument("--github-reviewer-general-token".into()))?
        .trim();
    if general_token.is_empty() {
        return Err(CliError::InvalidArgument(
            "--github-reviewer-general-token".into(),
        ));
    }
    let secperf_token = read_secret(
        Path::new(required(&options, "--github-reviewer-secperf-token")?),
        1024,
    )?;
    let secperf_token = std::str::from_utf8(&secperf_token)
        .map_err(|_| CliError::InvalidArgument("--github-reviewer-secperf-token".into()))?
        .trim();
    if secperf_token.is_empty() {
        return Err(CliError::InvalidArgument(
            "--github-reviewer-secperf-token".into(),
        ));
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
    let now = options
        .get("--now")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| CliError::InvalidArgument("--now".into()))
        })
        .transpose()?
        .map_or_else(current_time, Ok)?;
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
    let transport = UreqTransport::new(Duration::from_secs(20));
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
    let general_review_writer = GitHubWriter::new(
        transport.clone(),
        "https://api.github.com",
        general_token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let secperf_review_writer = GitHubWriter::new(
        transport,
        "https://api.github.com",
        secperf_token,
        4 * 1024 * 1024,
        10,
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let mut store = Store::open(required(&options, "--database")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    let intake = if policy.intake.enabled {
        serde_json::to_value(
            crate::reconcile_intake(&reader, &policy, &mut store, now, global_paused)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?,
        )
        .map_err(|error| CliError::Reconciliation(error.to_string()))?
    } else {
        json!({"result": "disabled"})
    };
    let takeover = crate::reconcile_takeover_once(&reader, &policy, &mut store, now)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let authorization = crate::reconcile_active_authorization(&reader, &policy, &mut store, now)
        .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let (result, ci) = if authorization.is_authorized() {
        let result =
            crate::ingest_completed_once(&mut store, &policy, required(&options, "--hermes")?)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?;
        let ci = crate::reconcile_ci_once(&reader, &policy, &mut store, now)
            .map_err(|error| CliError::Reconciliation(error.to_string()))?;
        (
            serde_json::to_value(result)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?,
            serde_json::to_value(ci)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?,
        )
    } else {
        let blocked = json!({"result": "authorization_blocked"});
        (blocked.clone(), blocked)
    };
    let draft_pull_request = crate::publish_draft_pull_request_once(
        &writer,
        &policy,
        &mut store,
        Path::new(required(&options, "--git-askpass")?),
        Path::new(required(&options, "--github-token")?),
        now,
        required(&options, "--owner")?,
        lease_seconds,
        authorization.is_authorized(),
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let plan_publication = crate::publish_plan_once(
        &writer,
        &policy,
        &mut store,
        now,
        required(&options, "--owner")?,
        lease_seconds,
        authorization.is_authorized(),
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let review_publication = crate::publish_reviews_once(
        &general_review_writer,
        &secperf_review_writer,
        &policy,
        &mut store,
        now,
        required(&options, "--owner")?,
        lease_seconds,
        authorization.is_authorized(),
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let final_preflight = crate::reconcile_final_preflight_once(
        &reader,
        &policy,
        &mut store,
        now,
        required(&options, "--owner")?,
        lease_seconds,
        authorization.is_authorized(),
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let merge = crate::reconcile_merge_once(
        &reader,
        &writer,
        &policy,
        &mut store,
        now,
        required(&options, "--owner")?,
        lease_seconds,
        authorization.is_authorized(),
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let disposition = crate::consume_disposition_once(
        &writer,
        &policy,
        &mut store,
        now,
        required(&options, "--owner")?,
        lease_seconds,
        authorization.is_authorized(),
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    let dispatch = crate::dispatch_once(
        &mut store,
        &policy,
        crate::DispatchCycleContext {
            skills_repository_commit: skills_commit,
            hermes_program: required(&options, "--hermes")?,
            owner: required(&options, "--owner")?,
            now,
            lease_seconds,
            authorization_valid: authorization.is_authorized(),
        },
    )
    .map_err(|error| CliError::Reconciliation(error.to_string()))?;
    Ok(json!({
        "ok": true,
        "result": "active",
        "observed_at": now,
        "repository": policy.repository.full_name(),
        "policy_revision": policy.revision,
        "worker_result": result,
        "ci": ci,
        "intake": intake,
        "takeover": takeover,
        "authorization": authorization,
        "draft_pull_request": draft_pull_request,
        "plan_publication": plan_publication,
        "review_publication": review_publication,
        "final_preflight": final_preflight,
        "merge": merge,
        "disposition": disposition,
        "dispatch": dispatch,
    }))
}

fn direct_worker_cycle(arguments: &[String]) -> Result<Value, CliError> {
    let options = options(
        arguments,
        &[
            "--policy",
            "--database",
            "--cursor",
            "--git",
            "--skills-root",
            "--owner",
        ],
        &["--now", "--lease-seconds"],
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
        .unwrap_or(
            crate::recommended_direct_lease_seconds(&policy)
                .map_err(|error| CliError::Reconciliation(error.to_string()))?,
        );
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
    let mut store = Store::open(required(&options, "--database")?)
        .map_err(|error| CliError::Ledger(error.to_string()))?;
    let result = crate::run_direct_worker_once_with(
        &mut store,
        &policy,
        &runtime,
        crate::DirectWorkerCycleContext {
            owner: required(&options, "--owner")?,
            now,
            lease_seconds,
            authorization_valid: true,
        },
    )
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
            workflow_version: 2,
            contract_version: 1,
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
    let options = options(arguments, &["--database"], &["--now"])?;
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
    if metadata.permissions().mode() & 0o077 != 0 {
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
