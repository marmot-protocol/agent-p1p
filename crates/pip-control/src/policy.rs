//! Strict, versioned repository policy loading.

use std::collections::BTreeSet;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Component, Path};

use pip_contracts::WorkerRole;
use pip_controller::{ExecutionKind, RolePolicy, WorkflowPolicy};
use pip_core::{ActorId, CasePolicy, IntakePolicy, MergeMode, PolicyRevision};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    pub id: u64,
    pub owner: String,
    pub name: String,
    pub default_branch: String,
}

impl RepositoryIdentity {
    #[must_use]
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeConfiguration {
    pub enabled: bool,
    pub paused: bool,
    pub label: String,
    pub trusted_actor_ids: Vec<u64>,
    pub excluded_issue_numbers: Vec<u64>,
    pub held_issue_numbers: Vec<u64>,
    pub repository_active_limit: u32,
    pub global_active_limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfiguration {
    pub automation_actor_id: Option<u64>,
    pub reviewer_general_actor_id: Option<u64>,
    pub reviewer_secperf_actor_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum MergeModeConfiguration {
    Shadow,
    Guarded,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MergeConfiguration {
    mode: MergeModeConfiguration,
    pub autonomous: bool,
    pub method: String,
}

impl MergeConfiguration {
    #[must_use]
    pub const fn is_shadow(&self) -> bool {
        matches!(self.mode, MergeModeConfiguration::Shadow)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum ExecutionConfiguration {
    Hermes,
    Direct,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfiguration {
    pub role: WorkerRole,
    pub profile: String,
    execution: ExecutionConfiguration,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub max_runtime: String,
    pub priority: u32,
    pub skills: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStorageConfiguration {
    pub require_distinct_filesystem: bool,
    pub minimum_free_bytes: u64,
    pub terminal_retention_seconds: u64,
}

impl RoleConfiguration {
    #[must_use]
    pub const fn is_hermes(&self) -> bool {
        matches!(self.execution, ExecutionConfiguration::Hermes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPolicy {
    pub policy_format: u32,
    pub revision: u64,
    pub repository: RepositoryIdentity,
    pub board: String,
    pub workflow_version: u32,
    pub checkout: String,
    pub workspace: String,
    pub artifacts: String,
    pub workspace_storage: WorkspaceStorageConfiguration,
    pub branch_prefix: String,
    pub github: GitHubConfiguration,
    pub intake: IntakeConfiguration,
    pub dispatch_enabled: bool,
    pub merge: MergeConfiguration,
    pub max_remediation_rounds: u32,
    pub max_case_elapsed_seconds: u64,
    pub max_provider_failures: u32,
    pub max_repeated_finding_fingerprint: u32,
    pub sensitive_scope_categories: Vec<String>,
    pub required_ci_contexts: Vec<String>,
    pub roles: Vec<RoleConfiguration>,
}

impl RepositoryPolicy {
    pub fn intake_policy(&self, global_paused: bool) -> IntakePolicy {
        IntakePolicy {
            revision: PolicyRevision::new(
                NonZeroU64::new(self.revision).expect("validated policy"),
            ),
            intake_enabled: self.intake.enabled,
            global_paused,
            repository_paused: self.intake.paused,
            required_label: self.intake.label.clone(),
            trusted_actor_ids: self
                .intake
                .trusted_actor_ids
                .iter()
                .map(|id| ActorId::new(NonZeroU64::new(*id).expect("validated actor")))
                .collect(),
            repository_active_limit: NonZeroU32::new(self.intake.repository_active_limit)
                .expect("validated repository limit"),
            global_active_limit: NonZeroU32::new(self.intake.global_active_limit)
                .expect("validated global limit"),
        }
    }

    pub fn workflow_policy(&self) -> Result<WorkflowPolicy, PolicyError> {
        let roles = self
            .roles
            .iter()
            .map(|role| RolePolicy {
                role: role.role,
                profile: role.profile.clone(),
                execution: match role.execution {
                    ExecutionConfiguration::Hermes => ExecutionKind::Hermes,
                    ExecutionConfiguration::Direct => ExecutionKind::Direct,
                },
                provider: role.provider.clone(),
                model: role.model.clone(),
                max_runtime: role.max_runtime.clone(),
                priority: role.priority,
                skills: role.skills.clone(),
            })
            .collect();
        WorkflowPolicy::new(&self.board, &self.workspace, &self.branch_prefix, roles)
            .and_then(|workflow| {
                workflow.with_sensitive_scope_categories(self.sensitive_scope_categories.clone())
            })
            .and_then(|workflow| workflow.with_max_provider_failures(self.max_provider_failures))
            .map_err(|error| PolicyError::Dispatch(error.to_string()))
    }

    #[must_use]
    pub fn case_policy(&self) -> CasePolicy {
        CasePolicy {
            revision: PolicyRevision::new(
                NonZeroU64::new(self.revision).expect("validated policy revision"),
            ),
            merge_mode: if self.merge.is_shadow() {
                MergeMode::Shadow
            } else {
                MergeMode::Guarded
            },
            max_remediation_rounds: NonZeroU32::new(self.max_remediation_rounds)
                .expect("validated remediation bound"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyError {
    Malformed(String),
    Invalid,
    Dispatch(String),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(formatter, "malformed repository policy: {error}"),
            Self::Invalid => formatter.write_str("invalid repository policy"),
            Self::Dispatch(error) => write!(formatter, "invalid dispatch policy: {error}"),
        }
    }
}

impl std::error::Error for PolicyError {}

pub fn load_repository_policy(bytes: &[u8]) -> Result<RepositoryPolicy, PolicyError> {
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err(PolicyError::Invalid);
    }
    let policy: RepositoryPolicy =
        serde_json::from_slice(bytes).map_err(|error| PolicyError::Malformed(error.to_string()))?;
    validate_policy(&policy)?;
    policy.workflow_policy().map_err(|_| PolicyError::Invalid)?;
    Ok(policy)
}

fn validate_policy(policy: &RepositoryPolicy) -> Result<(), PolicyError> {
    let trusted = policy
        .intake
        .trusted_actor_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let exclusions = policy
        .intake
        .excluded_issue_numbers
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let held = policy
        .intake
        .held_issue_numbers
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let ci = policy.required_ci_contexts.iter().collect::<BTreeSet<_>>();
    let sensitive_scope = policy
        .sensitive_scope_categories
        .iter()
        .collect::<BTreeSet<_>>();
    let github_actor_ids = [
        policy.github.automation_actor_id,
        policy.github.reviewer_general_actor_id,
        policy.github.reviewer_secperf_actor_id,
    ];
    let configured_github_actor_ids = github_actor_ids
        .into_iter()
        .flatten()
        .collect::<BTreeSet<_>>();
    let github_identities_valid = (configured_github_actor_ids.is_empty()
        && github_actor_ids.iter().all(Option::is_none))
        || (configured_github_actor_ids.len() == github_actor_ids.len()
            && github_actor_ids.iter().all(Option::is_some));
    let reasoning_bindings_valid = policy.roles.iter().all(|role| match role.execution {
        ExecutionConfiguration::Hermes => role
            .reasoning_effort
            .as_deref()
            .is_some_and(valid_reasoning_effort),
        ExecutionConfiguration::Direct => role.reasoning_effort.is_none(),
    });
    let valid = policy.policy_format == 1
        && policy.revision > 0
        && policy.repository.id > 0
        && valid_segment(&policy.repository.owner)
        && valid_segment(&policy.repository.name)
        && valid_git_ref(&policy.repository.default_branch)
        && valid_segment(&policy.board)
        && policy.workflow_version > 0
        && valid_absolute_path(&policy.checkout)
        && valid_absolute_path(&policy.workspace)
        && valid_absolute_path(&policy.artifacts)
        && disjoint_paths(&policy.checkout, &policy.workspace)
        && disjoint_paths(&policy.checkout, &policy.artifacts)
        && disjoint_paths(&policy.workspace, &policy.artifacts)
        && policy.workspace_storage.minimum_free_bytes > 0
        && policy.workspace_storage.terminal_retention_seconds > 0
        && valid_branch_prefix(&policy.branch_prefix)
        && configured_github_actor_ids.iter().all(|id| *id > 0)
        && github_identities_valid
        && (!policy.dispatch_enabled || github_actor_ids.iter().all(Option::is_some))
        && valid_segment(&policy.intake.label)
        && !trusted.is_empty()
        && trusted.len() == policy.intake.trusted_actor_ids.len()
        && trusted.iter().all(|id| *id > 0)
        && exclusions.len() == policy.intake.excluded_issue_numbers.len()
        && exclusions.iter().all(|issue| *issue > 0)
        && held.len() == policy.intake.held_issue_numbers.len()
        && held.iter().all(|issue| *issue > 0)
        && exclusions.is_disjoint(&held)
        && policy.intake.repository_active_limit > 0
        && policy.intake.global_active_limit > 0
        && !(policy.merge.is_shadow() && policy.merge.autonomous)
        && matches!(policy.merge.method.as_str(), "merge" | "squash" | "rebase")
        && policy.max_remediation_rounds > 0
        && policy.max_case_elapsed_seconds > 0
        && policy.max_provider_failures > 0
        && policy.max_repeated_finding_fingerprint > 0
        && sensitive_scope.len() == policy.sensitive_scope_categories.len()
        && sensitive_scope.iter().all(|category| {
            matches!(
                category.as_str(),
                "CRYPTOGRAPHY"
                    | "MLS_CGKA"
                    | "KEY_HANDLING"
                    | "TRUST_ANCHOR"
                    | "MEMBERSHIP_AUTHORIZATION"
                    | "ADMIN_AUTHORIZATION"
                    | "PUSH_PAYLOAD_CONTEXT"
            )
        })
        && reasoning_bindings_valid
        && ci.len() == policy.required_ci_contexts.len()
        && ci.iter().all(|context| valid_text(context, 256));
    if valid {
        Ok(())
    } else {
        Err(PolicyError::Invalid)
    }
}

fn valid_reasoning_effort(value: &str) -> bool {
    matches!(
        value,
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
    )
}

fn valid_absolute_path(value: &str) -> bool {
    if value.len() > 4096 || value.trim() != value || value == "/" {
        return false;
    }
    let path = Path::new(value);
    path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn disjoint_paths(left: &str, right: &str) -> bool {
    let left = Path::new(left);
    let right = Path::new(right);
    left != right && !left.starts_with(right) && !right.starts_with(left)
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_branch_prefix(value: &str) -> bool {
    value.starts_with("pip/")
        && value.ends_with('/')
        && value.trim_end_matches('/').split('/').all(valid_segment)
}

fn valid_git_ref(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("//")
        && value.split('/').all(valid_segment)
}

fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value.chars().all(|character| !character.is_control())
}
