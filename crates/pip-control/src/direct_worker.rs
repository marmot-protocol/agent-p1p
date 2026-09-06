//! Durable execution cycle for roles bound to a direct provider runtime.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use pip_contracts::{CaseIdentity, ReviewMode, WorkerBinding, WorkerResult, WorkerRole};
use pip_controller::{DirectTaskSpec, ExecutionKind};
use pip_executor::{
    CursorExecutor, CursorHealthProbe, CursorTask, ProcessRunner, ProviderProbeError,
};
use pip_store::{ClaimedEffect, StoredCase};
use serde_json::Map;
use sha2::{Digest, Sha256};

use crate::{PolicyError, RepositoryPolicy};

const RUN_EFFECT: &str = "RUN_DIRECT_WORKER";
const LEASE_RECOVERY_MARGIN_SECONDS: u64 = 120;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectWorkerRuntimeError {
    Unavailable(String),
    Failed(String),
}

impl fmt::Display for DirectWorkerRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(error) => {
                write!(formatter, "direct worker runtime unavailable: {error}")
            }
            Self::Failed(error) => write!(formatter, "direct worker execution failed: {error}"),
        }
    }
}

impl std::error::Error for DirectWorkerRuntimeError {}

pub trait DirectWorkerRuntime {
    fn execute(
        &self,
        task: &DirectTaskSpec,
        attempt_id: u64,
    ) -> Result<WorkerResult, DirectWorkerRuntimeError>;
}

pub fn recommended_direct_lease_seconds(
    policy: &RepositoryPolicy,
) -> Result<u64, DirectWorkerRuntimeError> {
    let max_runtime = policy
        .workflow_policy()
        .map_err(|error| runtime_error(error.to_string()))?
        .roles()
        .iter()
        .filter(|role| role.execution == ExecutionKind::Direct)
        .map(|role| parse_runtime(&role.max_runtime))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .ok_or_else(|| runtime_error("policy has no direct worker roles"))?;
    max_runtime
        .as_secs()
        .checked_add(LEASE_RECOVERY_MARGIN_SECONDS)
        .ok_or_else(|| runtime_error("direct worker lease overflow"))
}

pub struct CursorDirectRuntime<R> {
    runner: R,
    cursor_program: String,
    git_program: String,
    worktree_root: PathBuf,
    artifact_root: PathBuf,
    skills_root: PathBuf,
    environment: BTreeMap<String, String>,
    max_output_bytes: usize,
}

impl<R: ProcessRunner + Clone> CursorDirectRuntime<R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runner: R,
        cursor_program: impl Into<String>,
        git_program: impl Into<String>,
        worktree_root: impl AsRef<Path>,
        artifact_root: impl AsRef<Path>,
        skills_root: impl AsRef<Path>,
        environment: BTreeMap<String, String>,
        max_output_bytes: usize,
    ) -> Result<Self, DirectWorkerRuntimeError> {
        let cursor_program = cursor_program.into();
        let git_program = git_program.into();
        if cursor_program.trim().is_empty()
            || git_program.trim().is_empty()
            || max_output_bytes == 0
        {
            return Err(runtime_error("invalid direct runtime configuration"));
        }
        Ok(Self {
            runner,
            cursor_program,
            git_program,
            worktree_root: canonical_directory(worktree_root.as_ref())?,
            artifact_root: canonical_directory(artifact_root.as_ref())?,
            skills_root: canonical_directory(skills_root.as_ref())?,
            environment,
            max_output_bytes,
        })
    }

    fn execute_task(
        &self,
        task: &DirectTaskSpec,
        attempt_id: u64,
    ) -> Result<WorkerResult, DirectWorkerRuntimeError> {
        let worktree = canonical_directory(Path::new(&task.workspace))?;
        if worktree == self.worktree_root || !worktree.starts_with(&self.worktree_root) {
            return Err(runtime_error(
                "task worktree is outside its configured root",
            ));
        }
        let binding = task_binding(task)?;
        let workflow_skill = read_skill(
            &self.skills_root,
            Path::new("shared/workflow-contract/SKILL.md"),
        )?;
        let field_guide = read_skill(
            &self.skills_root,
            Path::new("shared/workflow-contract/references/worker-result-contracts.md"),
        )?;
        // Direct providers receive skill text rather than a Hermes skill path.
        // Inline the same packaged reference so no target-repo file is assumed.
        let workflow_skill =
            format!("{workflow_skill}\n\n# Worker result field guide\n\n{field_guide}");
        let role_skill = read_skill(
            &self.skills_root,
            Path::new(role_skill_name(task.role))
                .join("SKILL.md")
                .as_path(),
        )?;
        let timeout = parse_runtime(&task.max_runtime)?;
        let probe = CursorHealthProbe::new(
            self.runner.clone(),
            &self.cursor_program,
            worktree.clone(),
            self.environment.clone(),
            Duration::from_secs(30).min(timeout),
            self.max_output_bytes.min(1_048_576),
        )
        .map_err(provider_error)?;
        let health = probe.probe(&task.model).map_err(provider_error)?;
        let artifact_dir = attempt_artifact_dir(&self.artifact_root, &task.task_id, attempt_id)?;
        let executor = CursorExecutor::new(
            self.runner.clone(),
            &self.cursor_program,
            &self.git_program,
            self.environment.clone(),
            timeout,
            self.max_output_bytes,
        )
        .map_err(|error| runtime_error(error.to_string()))?;
        executor
            .execute(
                &health,
                &CursorTask {
                    binding,
                    immutable_input: task.body.clone(),
                    workflow_skill,
                    role_skill,
                },
                &worktree,
                &artifact_dir,
            )
            .map_err(|error| DirectWorkerRuntimeError::Failed(error.to_string()))
    }
}

impl<R: ProcessRunner + Clone> DirectWorkerRuntime for CursorDirectRuntime<R> {
    fn execute(
        &self,
        task: &DirectTaskSpec,
        attempt_id: u64,
    ) -> Result<WorkerResult, DirectWorkerRuntimeError> {
        self.execute_task(task, attempt_id)
    }
}

#[derive(Debug)]
pub(crate) enum DirectWorkerError {
    Policy(PolicyError),
    InvalidJob,
}

impl From<PolicyError> for DirectWorkerError {
    fn from(error: PolicyError) -> Self {
        Self::Policy(error)
    }
}

impl fmt::Display for DirectWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(error) => error.fmt(formatter),
            Self::InvalidJob => formatter.write_str("invalid direct worker job"),
        }
    }
}

pub(crate) fn validate_job(
    claimed: &ClaimedEffect,
    case: &StoredCase,
    task: &DirectTaskSpec,
    policy: &RepositoryPolicy,
) -> Result<WorkerBinding, DirectWorkerError> {
    let body = task.body.as_object().ok_or(DirectWorkerError::InvalidJob)?;
    let workflow = policy.workflow_policy()?;
    let reviewer_id = optional_text(body, "reviewer_id")?;
    let configured = if let Some(reviewer_id) = reviewer_id {
        workflow.reviewer(reviewer_id).ok()
    } else {
        workflow
            .roles()
            .iter()
            .find(|configured| configured.role == task.role && configured.reviewer_id.is_none())
    }
    .ok_or(DirectWorkerError::InvalidJob)?;
    let role = role_name(task.role);
    let worker_id = reviewer_id.unwrap_or(role);
    let expected_effect_id = format!("{}:direct:{worker_id}", task.source_effect_id);
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
        "{}:{worker_id}:round:{round}:revision:{}:worker",
        case.case_key, case.state_revision
    );
    let requested_model = format!("{}/{}", configured.provider, configured.model);
    let invalid_model = matches!(
        task.model.to_ascii_lowercase().as_str(),
        "auto" | "default" | "latest"
    );
    let expected_effect_type = if configured
        .review_mode
        .is_some_and(|mode| matches!(mode, ReviewMode::Advisory | ReviewMode::Shadow))
    {
        "RUN_DIRECT_OBSERVER"
    } else {
        RUN_EFFECT
    };
    let expected_review_mode = configured.review_mode.map(review_mode_name);
    let valid = task.schema_version == 1
        && claimed.effect_type == expected_effect_type
        && claimed.effect_id == expected_effect_id
        && claimed.case_key == case.case_key
        && claimed.state_revision == case.state_revision
        && case.repository_id == policy.repository.id
        && case.workflow_version == policy.workflow_version
        && task.task_id == expected_task_id
        && task.title == format!("Run {worker_id} for {}", case.case_key)
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
        && reviewer_id == configured.reviewer_id.as_deref()
        && optional_text(body, "review_mode")? == expected_review_mode
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
        reviewer_id: configured.reviewer_id.clone(),
        review_mode: configured.review_mode,
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

fn role_skill_name(role: WorkerRole) -> &'static str {
    match role {
        WorkerRole::Builder => "builder-grok",
        WorkerRole::ReviewerSecperf => "reviewer-secperf",
        WorkerRole::Planner => "planner",
        WorkerRole::ReviewerGeneral => "reviewer-general",
        WorkerRole::FinalReviewer => "final-reviewer",
    }
}

fn task_binding(task: &DirectTaskSpec) -> Result<WorkerBinding, DirectWorkerRuntimeError> {
    let body = task
        .body
        .as_object()
        .ok_or_else(|| runtime_error("direct task body is not an object"))?;
    let repository_id = runtime_number(body, "repository_id")?;
    let issue_number = runtime_number(body, "issue_number")?;
    let workflow_version = u32::try_from(runtime_number(body, "workflow_version")?)
        .map_err(|_| runtime_error("invalid workflow version"))?;
    let plan_version = u32::try_from(runtime_number(body, "plan_version")?)
        .map_err(|_| runtime_error("invalid plan version"))?;
    Ok(WorkerBinding {
        case: CaseIdentity {
            repository_id,
            issue_number,
            workflow_version,
        },
        task_id: task.task_id.clone(),
        role: task.role,
        reviewer_id: runtime_optional_text(body, "reviewer_id")?.map(str::to_owned),
        review_mode: runtime_optional_text(body, "review_mode")?
            .map(parse_review_mode)
            .transpose()?,
        requested_model: runtime_text(body, "requested_model")?.into(),
        skills_repository_commit: runtime_text(body, "skills_repository_commit")?.into(),
        plan_version,
        pr_number: runtime_optional_number(body, "pr_number")?,
        expected_head_sha: runtime_optional_text(body, "expected_head_sha")?.map(str::to_owned),
    })
}

fn runtime_number(
    body: &Map<String, serde_json::Value>,
    field: &str,
) -> Result<u64, DirectWorkerRuntimeError> {
    body.get(field)
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| runtime_error(format!("invalid direct task field {field}")))
}

fn runtime_optional_number(
    body: &Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<u64>, DirectWorkerRuntimeError> {
    body.get(field)
        .map(|_| runtime_number(body, field))
        .transpose()
}

fn runtime_text<'a>(
    body: &'a Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str, DirectWorkerRuntimeError> {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| runtime_error(format!("invalid direct task field {field}")))
}

fn runtime_optional_text<'a>(
    body: &'a Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<&'a str>, DirectWorkerRuntimeError> {
    body.get(field)
        .map(|_| runtime_text(body, field))
        .transpose()
}

fn parse_review_mode(value: &str) -> Result<ReviewMode, DirectWorkerRuntimeError> {
    match value {
        "required" => Ok(ReviewMode::Required),
        "advisory" => Ok(ReviewMode::Advisory),
        "shadow" => Ok(ReviewMode::Shadow),
        _ => Err(runtime_error("invalid review mode")),
    }
}

const fn review_mode_name(mode: ReviewMode) -> &'static str {
    match mode {
        ReviewMode::Required => "required",
        ReviewMode::Advisory => "advisory",
        ReviewMode::Shadow => "shadow",
    }
}

fn parse_runtime(value: &str) -> Result<Duration, DirectWorkerRuntimeError> {
    let minutes = value
        .strip_prefix("PT")
        .and_then(|value| value.strip_suffix('M'))
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|minutes| (1..=240).contains(minutes))
        .ok_or_else(|| runtime_error("invalid bounded direct runtime"))?;
    Ok(Duration::from_secs(minutes * 60))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, DirectWorkerRuntimeError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| runtime_error(format!("runtime directory unavailable: {error}")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(runtime_error("runtime path is not a real directory"));
    }
    path.canonicalize()
        .map_err(|error| runtime_error(format!("runtime directory unavailable: {error}")))
}

fn read_skill(root: &Path, relative: &Path) -> Result<String, DirectWorkerRuntimeError> {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| runtime_error(format!("canonical skill unavailable: {error}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 256 * 1024 {
        return Err(runtime_error(
            "canonical skill is not a bounded regular file",
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| runtime_error(format!("canonical skill unavailable: {error}")))?;
    if !canonical.starts_with(root) {
        return Err(runtime_error("canonical skill escapes the skills root"));
    }
    let value = fs::read_to_string(canonical)
        .map_err(|error| runtime_error(format!("canonical skill unavailable: {error}")))?;
    if value.trim().is_empty() {
        return Err(runtime_error("canonical skill is empty"));
    }
    Ok(value)
}

fn attempt_artifact_dir(
    root: &Path,
    task_id: &str,
    attempt_id: u64,
) -> Result<PathBuf, DirectWorkerRuntimeError> {
    if attempt_id == 0 {
        return Err(runtime_error("direct worker attempt id is required"));
    }
    let task_digest = hex_digest(&Sha256::digest(task_id.as_bytes()));
    let task_root = root.join(task_digest);
    if task_root.exists() {
        let metadata = fs::symlink_metadata(&task_root)
            .map_err(|error| runtime_error(format!("artifact root unavailable: {error}")))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(runtime_error("artifact task path is not a real directory"));
        }
    } else {
        fs::create_dir(&task_root)
            .map_err(|error| runtime_error(format!("artifact root unavailable: {error}")))?;
        #[cfg(unix)]
        fs::set_permissions(&task_root, fs::Permissions::from_mode(0o2750))
            .map_err(|error| runtime_error(format!("artifact root unavailable: {error}")))?;
    }
    let path = task_root.join(format!("attempt-{attempt_id:05}"));
    if path.exists() {
        return Err(runtime_error(
            "direct worker attempt artifacts already exist",
        ));
    }
    Ok(path)
}

fn provider_error(error: ProviderProbeError) -> DirectWorkerRuntimeError {
    runtime_error(error.to_string())
}

fn runtime_error(error: impl Into<String>) -> DirectWorkerRuntimeError {
    DirectWorkerRuntimeError::Unavailable(error.into())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
