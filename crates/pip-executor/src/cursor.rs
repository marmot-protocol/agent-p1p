//! Fresh direct Cursor worker execution.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use pip_contracts::{WorkerBinding, WorkerResult, WorkerRole};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    HealthAssurance, ProcessError, ProcessOutput, ProcessRunner, ProcessSpec, ProviderHealth,
};

#[derive(Clone, Debug, PartialEq)]
pub struct CursorTask {
    pub binding: WorkerBinding,
    pub immutable_input: Value,
    pub workflow_skill: String,
    pub role_skill: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CursorExecutionError {
    InvalidConfiguration,
    InvalidTask,
    InvalidWorktree,
    ArtifactExists,
    ArtifactIo(String),
    TemporaryIo(String),
    UnsafeSecretInput,
    UnsafeSecretOutput,
    HealthBindingMismatch,
    Process(ProcessError),
    TimedOut,
    OutputTooLarge,
    CommandFailed(i32),
    InvalidUtf8,
    MalformedEnvelope(String),
    InvalidResult(String),
    InvalidResultArtifact(String),
    ResultConflict,
    ReviewerDirtyBaseline,
    ReviewerHeadMismatch,
    ReviewerMutation,
}

impl fmt::Display for CursorExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("invalid Cursor executor configuration")
            }
            Self::InvalidTask => formatter.write_str("invalid direct Cursor task"),
            Self::InvalidWorktree => formatter.write_str("invalid assigned worktree"),
            Self::ArtifactExists => formatter.write_str("worker artifact directory already exists"),
            Self::ArtifactIo(error) => write!(formatter, "worker artifact I/O failed: {error}"),
            Self::TemporaryIo(error) => {
                write!(formatter, "worker temporary storage failed: {error}")
            }
            Self::UnsafeSecretInput => {
                formatter.write_str("worker input contains credential-like material")
            }
            Self::UnsafeSecretOutput => {
                formatter.write_str("worker output contains credential-like material")
            }
            Self::HealthBindingMismatch => {
                formatter.write_str("provider health does not match the exact task model")
            }
            Self::Process(error) => error.fmt(formatter),
            Self::TimedOut => formatter.write_str("direct Cursor worker timed out"),
            Self::OutputTooLarge => {
                formatter.write_str("direct Cursor worker output exceeded its bound")
            }
            Self::CommandFailed(status) => {
                write!(formatter, "direct Cursor worker exited with {status}")
            }
            Self::InvalidUtf8 => formatter.write_str("direct Cursor worker output is not UTF-8"),
            Self::MalformedEnvelope(error) => {
                write!(formatter, "malformed Cursor result envelope: {error}")
            }
            Self::InvalidResult(error) => write!(formatter, "invalid bound worker result: {error}"),
            Self::InvalidResultArtifact(error) => {
                write!(formatter, "invalid durable worker result artifact: {error}")
            }
            Self::ResultConflict => {
                formatter.write_str("Cursor stdout and durable worker result artifact disagree")
            }
            Self::ReviewerDirtyBaseline => {
                formatter.write_str("reviewer worktree was dirty before execution")
            }
            Self::ReviewerHeadMismatch => {
                formatter.write_str("reviewer worktree is not at the bound exact head")
            }
            Self::ReviewerMutation => {
                formatter.write_str("read-only reviewer modified its assigned worktree")
            }
        }
    }
}

impl std::error::Error for CursorExecutionError {}

impl From<ProcessError> for CursorExecutionError {
    fn from(error: ProcessError) -> Self {
        Self::Process(error)
    }
}

pub struct CursorExecutor<R> {
    runner: R,
    program: String,
    git_program: String,
    environment: BTreeMap<String, String>,
    timeout: Duration,
    max_output_bytes: usize,
}

impl<R: ProcessRunner> CursorExecutor<R> {
    pub fn new(
        runner: R,
        program: impl Into<String>,
        git_program: impl Into<String>,
        environment: BTreeMap<String, String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<Self, CursorExecutionError> {
        let program = program.into();
        let git_program = git_program.into();
        if program.trim().is_empty()
            || git_program.trim().is_empty()
            || timeout.is_zero()
            || max_output_bytes == 0
        {
            return Err(CursorExecutionError::InvalidConfiguration);
        }
        Ok(Self {
            runner,
            program,
            git_program,
            environment,
            timeout,
            max_output_bytes,
        })
    }

    pub fn execute(
        &self,
        health: &ProviderHealth,
        task: &CursorTask,
        worktree: &Path,
        artifact_dir: &Path,
    ) -> Result<WorkerResult, CursorExecutionError> {
        self.validate_health(health, task)?;
        validate_task(task)?;
        let worktree = worktree
            .canonicalize()
            .map_err(|_| CursorExecutionError::InvalidWorktree)?;
        if !worktree.is_dir() {
            return Err(CursorExecutionError::InvalidWorktree);
        }
        // Service-private /tmp is deliberately independent of the long case and
        // artifact paths. Unix socket tests need space for their own filenames.
        // TempDir removes only this fresh allocation on every return path.
        let mut temporary_builder = tempfile::Builder::new();
        temporary_builder.prefix("pip-");
        #[cfg(unix)]
        temporary_builder.permissions(fs::Permissions::from_mode(0o700));
        let temporary = temporary_builder
            .tempdir_in("/tmp")
            .map_err(|error| CursorExecutionError::TemporaryIo(error.to_string()))?;
        let mut environment = crate::workspace_git_environment(&worktree, self.environment.clone());
        for key in ["TMPDIR", "TMP", "TEMP"] {
            environment.insert(key.into(), temporary.path().display().to_string());
        }
        let mut immutable_input = task.immutable_input.clone();
        let evidence = immutable_input
            .as_object_mut()
            .and_then(|input| input.remove("immutable_evidence_bundle"))
            .map(|bundle| {
                let compact = serde_json::to_vec(&bundle)
                    .map_err(|error| CursorExecutionError::InvalidResult(error.to_string()))?;
                if compact.len() > pip_contracts::MAX_EVIDENCE_BUNDLE_BYTES {
                    return Err(CursorExecutionError::UnsafeSecretInput);
                }
                serde_json::to_vec_pretty(&bundle)
                    .map_err(|error| CursorExecutionError::InvalidResult(error.to_string()))
            })
            .transpose()?;
        if let Some(bytes) = &evidence {
            if bytes.len() > pip_contracts::MAX_EVIDENCE_ARTIFACT_BYTES
                || contains_secret(&String::from_utf8_lossy(bytes))
            {
                return Err(CursorExecutionError::UnsafeSecretInput);
            }
            let digest: String = Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            immutable_input["immutable_evidence_ref"] = json!({
                "schema_version":1,
                "path":artifact_dir.join("immutable-evidence.json"),
                "sha256":digest,
            });
        }
        let task_input = serde_json::to_string_pretty(&json!({
            "binding": &task.binding,
            "input": immutable_input,
        }))
        .map_err(|error| CursorExecutionError::InvalidResult(error.to_string()))?
            + "\n";
        let prompt = render_prompt(task, &task_input, artifact_dir, temporary.path());
        if task_input.len() > self.max_output_bytes
            || prompt.len() > self.max_output_bytes
            || contains_secret(&task_input)
            || contains_secret(&prompt)
        {
            return Err(CursorExecutionError::UnsafeSecretInput);
        }

        create_artifact_dir(artifact_dir)?;
        write_json(
            artifact_dir,
            "run-status.json",
            &json!({"status": "INCOMPLETE"}),
        )?;
        write_artifact(artifact_dir, "task-input.json", task_input.as_bytes())?;
        if let Some(bytes) = &evidence {
            write_artifact(artifact_dir, "immutable-evidence.json", bytes)?;
        }
        write_artifact(artifact_dir, "prompt.md", prompt.as_bytes())?;

        // Both roles need shell checks and evidence writes without an operator
        // prompt. This is command approval, not an OS read-only boundary. The
        // service isolates credentials; reviews must also pass the unchanged
        // exact-head checkout postcondition before their result is accepted.
        let mut args = vec!["--print".into(), "--trust".into(), "--force".into()];
        // Prompt bytes travel through stdin, not an OS-size-limited argument.
        args.extend([
            "--output-format".into(),
            "json".into(),
            "--model".into(),
            health.model.clone(),
        ]);
        write_json(
            artifact_dir,
            "invocation.json",
            &json!({
                "provider": health.provider,
                "model": health.model,
                "role": task.binding.role,
                "worktree": worktree,
                "fresh_session": true,
                "command": args,
                "stdin": "prompt.md",
                "environment_keys": environment.keys().collect::<Vec<_>>(),
                "temporary_directory": temporary.path(),
                "timeout_seconds": self.timeout.as_secs(),
                "max_output_bytes": self.max_output_bytes,
            }),
        )?;
        write_json(
            artifact_dir,
            "model-verification.json",
            &json!({
                "provider": health.provider,
                "requested_model": health.model,
                "provider_version": health.version,
                "assurance": "ADVERTISED_EXACT_REQUEST_WITHOUT_ROUTE_ATTESTATION",
                "cli_reports_actual_route": false,
            }),
        )?;

        let reviewer_before = if matches!(
            task.binding.role,
            WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
        ) {
            let snapshot = self.git_snapshot(&worktree)?;
            if !snapshot.status.is_empty() {
                return Err(CursorExecutionError::ReviewerDirtyBaseline);
            }
            if task.binding.expected_head_sha.as_deref() != Some(snapshot.head.as_str()) {
                return Err(CursorExecutionError::ReviewerHeadMismatch);
            }
            Some(snapshot)
        } else {
            None
        };

        let output = self.runner.run(&ProcessSpec {
            program: self.program.clone(),
            args,
            stdin_file: Some(artifact_dir.join("prompt.md")),
            cwd: worktree.clone(),
            environment,
            timeout: self.timeout,
            max_output_bytes: self.max_output_bytes,
        })?;
        let unsafe_output = contains_secret(&String::from_utf8_lossy(&output.stdout))
            || contains_secret(&String::from_utf8_lossy(&output.stderr));
        if unsafe_output {
            write_artifact(
                artifact_dir,
                "stdout.log",
                b"[REDACTED unsafe provider output]\n",
            )?;
            write_artifact(
                artifact_dir,
                "stderr.log",
                b"[REDACTED unsafe provider output]\n",
            )?;
            return Err(CursorExecutionError::UnsafeSecretOutput);
        }
        write_artifact(artifact_dir, "stdout.log", &output.stdout)?;
        write_artifact(artifact_dir, "stderr.log", &output.stderr)?;

        if let Some(before) = reviewer_before {
            let after = self.git_snapshot(&worktree)?;
            if after != before {
                return Err(CursorExecutionError::ReviewerMutation);
            }
        }

        let transport_error = validate_output(&output, self.max_output_bytes).err();
        let stdout_result = if output.stdout.len() <= self.max_output_bytes
            && output.stderr.len() <= self.max_output_bytes
        {
            parse_envelope(&output.stdout).and_then(|(result, envelope)| {
                result
                    .validate_binding(&task.binding)
                    .map_err(|error| CursorExecutionError::InvalidResult(error.to_string()))?;
                Ok((result, envelope))
            })
        } else {
            Err(CursorExecutionError::OutputTooLarge)
        };
        let artifact_result =
            match read_result_artifact(artifact_dir, self.max_output_bytes, &task.binding) {
                Ok(result) => result,
                Err(error) => {
                    write_execution_outcome(artifact_dir, "INVALID_RESULT_ARTIFACT", &output)?;
                    return Err(error);
                }
            };

        let (result, envelope, classification) = match (stdout_result, artifact_result) {
            (Ok((stdout, _envelope)), Some(artifact)) if stdout != artifact => {
                write_execution_outcome(artifact_dir, "RESULT_CONFLICT", &output)?;
                return Err(CursorExecutionError::ResultConflict);
            }
            (Ok((stdout, envelope)), Some(_)) => {
                let classification = match transport_error {
                    Some(CursorExecutionError::TimedOut) => "ARTIFACT_RECOVERY_AFTER_TIMEOUT",
                    Some(CursorExecutionError::CommandFailed(_)) => {
                        "ARTIFACT_RECOVERY_AFTER_PROVIDER_FAILURE"
                    }
                    Some(_) => "ARTIFACT_RECOVERY_AFTER_TRANSPORT_FAILURE",
                    None => "STDOUT_AND_ARTIFACT_RESULT",
                };
                (stdout, Some(envelope), classification)
            }
            (Err(_), Some(artifact)) => {
                let classification = match transport_error {
                    Some(CursorExecutionError::TimedOut) => "ARTIFACT_RECOVERY_AFTER_TIMEOUT",
                    Some(CursorExecutionError::CommandFailed(_)) => {
                        "ARTIFACT_RECOVERY_AFTER_PROVIDER_FAILURE"
                    }
                    Some(_) => "ARTIFACT_RECOVERY_AFTER_TRANSPORT_FAILURE",
                    None => "ARTIFACT_RECOVERY_AFTER_INVALID_STDOUT",
                };
                (artifact, None, classification)
            }
            (Ok((stdout, envelope)), None) => {
                if let Some(error) = transport_error {
                    let classification = match error {
                        CursorExecutionError::TimedOut => "TIMEOUT_WITHOUT_DURABLE_RESULT",
                        CursorExecutionError::CommandFailed(_) => {
                            "PROVIDER_FAILURE_WITHOUT_DURABLE_RESULT"
                        }
                        _ => "TRANSPORT_FAILURE_WITHOUT_DURABLE_RESULT",
                    };
                    write_execution_outcome(artifact_dir, classification, &output)?;
                    return Err(error);
                }
                (stdout, Some(envelope), "STDOUT_RESULT")
            }
            (Err(stdout_error), None) => {
                let error = transport_error.unwrap_or(stdout_error);
                let classification = match error {
                    CursorExecutionError::TimedOut => "TIMEOUT_NO_RESULT",
                    CursorExecutionError::CommandFailed(_) => "PROVIDER_FAILURE_NO_RESULT",
                    CursorExecutionError::OutputTooLarge => "OUTPUT_TOO_LARGE_NO_RESULT",
                    _ => "INVALID_STDOUT_NO_RESULT",
                };
                write_execution_outcome(artifact_dir, classification, &output)?;
                return Err(error);
            }
        };

        if let Some(envelope) = envelope {
            write_json(artifact_dir, "cursor-envelope.json", &envelope)?;
        }
        write_json(artifact_dir, "result.json", &result)?;
        write_execution_outcome(artifact_dir, classification, &output)?;
        write_json(
            artifact_dir,
            "run-status.json",
            &json!({"status": "COMPLETE"}),
        )?;
        Ok(result)
    }

    fn validate_health(
        &self,
        health: &ProviderHealth,
        task: &CursorTask,
    ) -> Result<(), CursorExecutionError> {
        if health.provider != "cursor"
            || health.assurance != HealthAssurance::AdvertisedExact
            || task.binding.requested_model != format!("cursor/{}", health.model)
        {
            return Err(CursorExecutionError::HealthBindingMismatch);
        }
        Ok(())
    }

    fn git_snapshot(&self, worktree: &Path) -> Result<GitSnapshot, CursorExecutionError> {
        let head = self.run_git(worktree, vec!["rev-parse".into(), "HEAD".into()])?;
        let head = String::from_utf8(head.stdout)
            .map_err(|_| CursorExecutionError::InvalidUtf8)?
            .trim()
            .to_owned();
        let status = self.run_git(
            worktree,
            vec![
                "status".into(),
                "--porcelain=v1".into(),
                "--untracked-files=all".into(),
            ],
        )?;
        let status =
            String::from_utf8(status.stdout).map_err(|_| CursorExecutionError::InvalidUtf8)?;
        Ok(GitSnapshot { head, status })
    }

    fn run_git(
        &self,
        worktree: &Path,
        args: Vec<String>,
    ) -> Result<ProcessOutput, CursorExecutionError> {
        let output = self.runner.run(&ProcessSpec {
            program: self.git_program.clone(),
            args,
            stdin_file: None,
            cwd: worktree.to_owned(),
            environment: crate::workspace_git_environment(worktree, self.environment.clone()),
            timeout: Duration::from_secs(30).min(self.timeout),
            max_output_bytes: self.max_output_bytes.min(1_048_576),
        })?;
        validate_output(&output, self.max_output_bytes.min(1_048_576))?;
        Ok(output)
    }
}

fn read_result_artifact(
    artifact_dir: &Path,
    max_bytes: usize,
    binding: &WorkerBinding,
) -> Result<Option<WorkerResult>, CursorExecutionError> {
    let path = artifact_dir.join("worker-result.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CursorExecutionError::InvalidResultArtifact(
                error.to_string(),
            ));
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CursorExecutionError::InvalidResultArtifact(
            "worker-result.json is not a regular non-symlink file".into(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > max_bytes as u64 {
        return Err(CursorExecutionError::InvalidResultArtifact(
            "worker-result.json has an invalid size".into(),
        ));
    }
    let file = File::open(&path)
        .map_err(|error| CursorExecutionError::InvalidResultArtifact(error.to_string()))?;
    let opened = file
        .metadata()
        .map_err(|error| CursorExecutionError::InvalidResultArtifact(error.to_string()))?;
    #[cfg(unix)]
    if metadata.dev() != opened.dev()
        || metadata.ino() != opened.ino()
        || metadata.len() != opened.len()
    {
        return Err(CursorExecutionError::InvalidResultArtifact(
            "worker-result.json changed while it was opened".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CursorExecutionError::InvalidResultArtifact(error.to_string()))?;
    if bytes.len() != opened.len() as usize || bytes.len() > max_bytes {
        return Err(CursorExecutionError::InvalidResultArtifact(
            "worker-result.json changed while it was read".into(),
        ));
    }
    if contains_secret(&String::from_utf8_lossy(&bytes)) {
        return Err(CursorExecutionError::InvalidResultArtifact(
            "worker-result.json contains credential-like material".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CursorExecutionError::InvalidResultArtifact(error.to_string()))?;
    let result = WorkerResult::decode(value)
        .map_err(|error| CursorExecutionError::InvalidResultArtifact(error.to_string()))?;
    result
        .validate_binding(binding)
        .map_err(|error| CursorExecutionError::InvalidResultArtifact(error.to_string()))?;
    Ok(Some(result))
}

fn write_execution_outcome(
    artifact_dir: &Path,
    classification: &str,
    output: &ProcessOutput,
) -> Result<(), CursorExecutionError> {
    write_json(
        artifact_dir,
        "execution-outcome.json",
        &json!({
            "classification": classification,
            "process_status": output.status,
            "timed_out": output.timed_out,
            "stdout_bytes": output.stdout.len(),
            "stderr_bytes": output.stderr.len(),
        }),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitSnapshot {
    head: String,
    status: String,
}

fn validate_task(task: &CursorTask) -> Result<(), CursorExecutionError> {
    if !matches!(
        task.binding.role,
        WorkerRole::Builder | WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
    ) || !task.immutable_input.is_object()
        || task.workflow_skill.trim().is_empty()
        || task.role_skill.trim().is_empty()
        || task.binding.task_id.trim().is_empty()
        || task.binding.plan_version == 0
    {
        return Err(CursorExecutionError::InvalidTask);
    }
    Ok(())
}

fn render_prompt(
    task: &CursorTask,
    task_input: &str,
    artifact_dir: &Path,
    temporary: &Path,
) -> String {
    let mut prompt = format!(
        "{}\n\n{}\n\n# Immutable Task Input\n\n```json\n{}```\n\n# Result Requirement\n\nReturn only the review-ready structured result contract bound to this exact task. Set requested_model to `{}` and report the model identity visible in the runtime as actual_model. A mismatch must use BLOCKED_UNEXPECTED_MODEL. Do not resume or reuse any prior session.\n\nRun artifact directory: `{}`. Save your contract there as `worker-result.json`, outside the source checkout, and validate it with `/opt/pip/current/bin/pip-control validate-worker-result --input <absolute-path-to-worker-result.json>` before returning the same JSON object. The direct runtime captures your response; no Hermes completion tool is needed.\n",
        task.workflow_skill,
        task.role_skill,
        task_input,
        task.binding.requested_model,
        artifact_dir.display(),
    );
    prompt.push_str(&format!(
        "\nTemporary storage: `{}` is assigned through TMPDIR, TMP and TEMP. Preserve these variables for tests and Unix sockets; do not replace them with a long case/artifact path. This directory is private, disposable and removed after this run. Keep build caches in the assigned managed workspace and retained evidence in the run artifact directory, never in temporary storage.\n",
        temporary.display(),
    ));
    prompt
}

fn validate_output(
    output: &ProcessOutput,
    max_output_bytes: usize,
) -> Result<(), CursorExecutionError> {
    if output.timed_out {
        return Err(CursorExecutionError::TimedOut);
    }
    if output.stdout.len() > max_output_bytes || output.stderr.len() > max_output_bytes {
        return Err(CursorExecutionError::OutputTooLarge);
    }
    if output.status != 0 {
        return Err(CursorExecutionError::CommandFailed(output.status));
    }
    Ok(())
}

#[derive(Deserialize)]
struct CursorEnvelope {
    #[serde(rename = "type")]
    kind: String,
    subtype: String,
    is_error: bool,
    result: Value,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

fn parse_envelope(stdout: &[u8]) -> Result<(WorkerResult, Value), CursorExecutionError> {
    let envelope_value: Value = serde_json::from_slice(stdout)
        .map_err(|error| CursorExecutionError::MalformedEnvelope(error.to_string()))?;
    let envelope: CursorEnvelope = serde_json::from_value(envelope_value.clone())
        .map_err(|error| CursorExecutionError::MalformedEnvelope(error.to_string()))?;
    if envelope.kind != "result" || envelope.subtype != "success" || envelope.is_error {
        return Err(CursorExecutionError::MalformedEnvelope(
            "envelope does not report a successful result".into(),
        ));
    }
    let _ = envelope.extra;
    let result = match envelope.result {
        Value::String(text) => WorkerResult::decode(result_from_transcript(&text)?),
        value @ Value::Object(_) => WorkerResult::decode(value),
        _ => {
            return Err(CursorExecutionError::MalformedEnvelope(
                "result is neither an object nor encoded object".into(),
            ));
        }
    }
    .map_err(|error| CursorExecutionError::InvalidResult(error.to_string()))?;
    Ok((result, envelope_value))
}

// Cursor's JSON envelope concatenates assistant progress and final messages.
// Locate exactly one top-level contract object, preserving its bytes/fields;
// never repair JSON, choose between competing answers, or weaken task binding.
// A single linear scan handles braces inside JSON strings without repeatedly
// parsing suffixes of a potentially large transcript.
fn result_from_transcript(text: &str) -> Result<Value, CursorExecutionError> {
    let invalid = || {
        CursorExecutionError::InvalidResult(
            "expected exactly one complete worker contract in Cursor transcript".into(),
        )
    };
    let mut found = None;
    let mut start = 0;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if depth == 0 {
            // Progress may quote Rust interpolation or code, e.g. `{err}`.
            // A JSON object starts with a quoted member name or is empty;
            // those prose braces are not competing result objects.
            if byte == b'{'
                && matches!(
                    text[index + 1..]
                        .bytes()
                        .find(|byte| !byte.is_ascii_whitespace()),
                    Some(b'"' | b'}')
                )
            {
                start = index;
                depth = 1;
            }
            continue;
        }
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let value: Value =
                        serde_json::from_str(&text[start..=index]).map_err(|_| invalid())?;
                    if value.get("contract_version").is_some() {
                        if found.is_some() {
                            return Err(invalid());
                        }
                        found = Some(value);
                    }
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(invalid());
    }
    found.ok_or_else(invalid)
}

fn create_artifact_dir(path: &Path) -> Result<(), CursorExecutionError> {
    if path.exists() {
        return Err(CursorExecutionError::ArtifactExists);
    }
    fs::create_dir(path).map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o750))
        .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
    sync_directory(path.parent().ok_or(CursorExecutionError::InvalidTask)?)?;
    Ok(())
}

fn write_json(
    directory: &Path,
    name: &str,
    value: &impl serde::Serialize,
) -> Result<(), CursorExecutionError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
    bytes.push(b'\n');
    write_artifact(directory, name, &bytes)
}

fn write_artifact(
    directory: &Path,
    name: &str,
    content: &[u8],
) -> Result<(), CursorExecutionError> {
    if name.is_empty() || name.contains('/') {
        return Err(CursorExecutionError::ArtifactIo(
            "invalid artifact name".into(),
        ));
    }
    let path = directory.join(name);
    let temporary = directory.join(format!(".{name}.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o640);
    let mut file = options
        .open(&temporary)
        .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
    let result = (|| {
        file.set_permissions(fs::Permissions::from_mode(0o640))
            .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
        file.write_all(content)
            .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
        file.sync_all()
            .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
        fs::rename(&temporary, &path)
            .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_directory(path: &Path) -> Result<(), CursorExecutionError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| CursorExecutionError::ArtifactIo(error.to_string()))
}

fn contains_secret(text: &str) -> bool {
    if text.contains("PRIVATE KEY") {
        return true;
    }
    text.split(|character: char| character.is_whitespace() || matches!(character, '"' | '\''))
        .any(|token| {
            (token.starts_with("ghp_") && token.len() >= 24)
                || (token.starts_with("gho_") && token.len() >= 24)
                || (token.starts_with("sk-") && token.len() >= 24)
                || (token.starts_with("AKIA") && token.len() == 20)
                || (token.starts_with("nsec1") && token.len() >= 25)
        })
}
