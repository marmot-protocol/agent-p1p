//! Policy-driven worker dispatch projection.

use std::collections::BTreeSet;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::WorkerRole;
use pip_core::{
    CaseId, Effect, GitSha, IssueNumber, PlanVersion, PullRequestNumber, RepositoryId,
    StateRevision, WorkflowVersion,
};
use pip_hermes::{GateCreateSpec, TaskCreateSpec};
use pip_store::{ClaimedEffect, StoredCase};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionKind {
    Hermes,
    Direct,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolePolicy {
    pub role: WorkerRole,
    pub profile: String,
    pub execution: ExecutionKind,
    pub provider: String,
    pub model: String,
    pub max_runtime: String,
    pub priority: u32,
    pub skills: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowPolicy {
    board: String,
    workspace: String,
    roles: Vec<RolePolicy>,
}

impl WorkflowPolicy {
    pub fn new(
        board: impl Into<String>,
        workspace: impl Into<String>,
        roles: Vec<RolePolicy>,
    ) -> Result<Self, DispatchError> {
        let board = board.into();
        let workspace = workspace.into();
        let expected = [
            WorkerRole::Planner,
            WorkerRole::Builder,
            WorkerRole::ReviewerGeneral,
            WorkerRole::ReviewerSecperf,
            WorkerRole::FinalReviewer,
        ];
        let unique = roles
            .iter()
            .map(|role| role_name(role.role))
            .collect::<BTreeSet<_>>();
        let valid_roles = roles.len() == expected.len()
            && unique.len() == expected.len()
            && expected
                .iter()
                .all(|expected| roles.iter().any(|role| role.role == *expected));
        let valid_bindings = roles.iter().all(valid_role_policy);
        if !valid_id(&board) || !valid_text(&workspace, 4096) || !valid_roles || !valid_bindings {
            return Err(DispatchError::InvalidPolicy);
        }
        Ok(Self {
            board,
            workspace,
            roles,
        })
    }

    #[must_use]
    pub fn roles(&self) -> &[RolePolicy] {
        &self.roles
    }

    fn role(&self, role: WorkerRole) -> Result<&RolePolicy, DispatchError> {
        self.roles
            .iter()
            .find(|binding| binding.role == role)
            .ok_or(DispatchError::InvalidPolicy)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchContext {
    pub case_id: CaseId,
    pub state_revision: StateRevision,
    pub plan_version: Option<PlanVersion>,
    pub remediation_round: u32,
    pub pr_number: Option<PullRequestNumber>,
    pub head_sha: Option<GitSha>,
    pub skills_repository_commit: GitSha,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDispatch {
    pub role: WorkerRole,
    pub gate: GateCreateSpec,
    pub worker_projection_key: String,
    pub worker_body: Value,
    worker_effect_id: String,
    worker_title: String,
    profile: String,
    workspace: String,
    skills: Vec<String>,
    provider: String,
    model: String,
    max_runtime: String,
    priority: u32,
    board: String,
}

impl WorkflowDispatch {
    pub fn bind_gate(&self, gate_task_id: &str) -> Result<TaskCreateSpec, DispatchError> {
        if !valid_id(gate_task_id) {
            return Err(DispatchError::InvalidGateTask);
        }
        Ok(TaskCreateSpec {
            board: self.board.clone(),
            effect_id: self.worker_effect_id.clone(),
            projection_key: self.worker_projection_key.clone(),
            title: self.worker_title.clone(),
            body: self.worker_body.clone(),
            assignee: self.profile.clone(),
            workspace: self.workspace.clone(),
            skills: self.skills.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            max_runtime: self.max_runtime.clone(),
            priority: self.priority,
            parent_task_ids: vec![gate_task_id.into()],
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DispatchError {
    InvalidPolicy,
    InvalidEffectId,
    MissingPlan,
    MissingPullRequest,
    MissingExactHead,
    InvalidGateTask,
    InvalidStoredCase,
    StaleEffect,
    UnsupportedEffect,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPolicy => "invalid workflow role policy",
            Self::InvalidEffectId => "invalid durable effect identity",
            Self::MissingPlan => "worker dispatch requires an active plan",
            Self::MissingPullRequest => "review dispatch requires a pull request",
            Self::MissingExactHead => "review dispatch requires an exact head",
            Self::InvalidGateTask => "invalid activation gate task identity",
            Self::InvalidStoredCase => "ledger case cannot form a dispatch binding",
            Self::StaleEffect => "outbox effect does not match the current case revision",
            Self::UnsupportedEffect => "outbox effect is not a worker dispatch",
        })
    }
}

pub fn schedule_claimed_dispatch(
    claimed: &ClaimedEffect,
    case: &StoredCase,
    policy: &WorkflowPolicy,
    skills_repository_commit: GitSha,
) -> Result<Vec<WorkflowDispatch>, DispatchError> {
    if claimed.case_key != case.case_key || claimed.state_revision != case.state_revision {
        return Err(DispatchError::StaleEffect);
    }
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(DispatchError::InvalidStoredCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(DispatchError::InvalidStoredCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(DispatchError::InvalidStoredCase)?,
        ),
    );
    if case_id.to_string() != case.case_key {
        return Err(DispatchError::InvalidStoredCase);
    }
    let effect = match claimed.effect_type.as_str() {
        "DISPATCH_PLANNER" => Effect::DispatchPlanner,
        "DISPATCH_BUILDER" => Effect::DispatchBuilder,
        "DISPATCH_REVIEWERS" => Effect::DispatchReviewers,
        "DISPATCH_FINAL_REVIEWER" => Effect::DispatchFinalReviewer,
        _ => return Err(DispatchError::UnsupportedEffect),
    };
    let context = DispatchContext {
        case_id,
        state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(DispatchError::InvalidStoredCase)?,
        ),
        plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
        remediation_round: case.remediation_round,
        pr_number: case
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: case
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| DispatchError::InvalidStoredCase)?,
        skills_repository_commit,
    };
    schedule_effect(&claimed.effect_id, effect, &context, policy)
}

impl std::error::Error for DispatchError {}

pub fn schedule_effect(
    effect_id: &str,
    effect: Effect,
    context: &DispatchContext,
    policy: &WorkflowPolicy,
) -> Result<Vec<WorkflowDispatch>, DispatchError> {
    if !valid_opaque(effect_id, 512) {
        return Err(DispatchError::InvalidEffectId);
    }
    match effect {
        Effect::DispatchPlanner => Ok(vec![dispatch(
            effect_id,
            WorkerRole::Planner,
            context,
            policy,
        )?]),
        Effect::DispatchBuilder => {
            context.plan_version.ok_or(DispatchError::MissingPlan)?;
            Ok(vec![dispatch(
                effect_id,
                WorkerRole::Builder,
                context,
                policy,
            )?])
        }
        Effect::DispatchReviewers => {
            require_review_binding(context)?;
            Ok(vec![
                dispatch(
                    &format!("{effect_id}:general"),
                    WorkerRole::ReviewerGeneral,
                    context,
                    policy,
                )?,
                dispatch(
                    &format!("{effect_id}:secperf"),
                    WorkerRole::ReviewerSecperf,
                    context,
                    policy,
                )?,
            ])
        }
        Effect::DispatchFinalReviewer => {
            require_review_binding(context)?;
            Ok(vec![dispatch(
                effect_id,
                WorkerRole::FinalReviewer,
                context,
                policy,
            )?])
        }
        _ => Ok(Vec::new()),
    }
}

fn require_review_binding(context: &DispatchContext) -> Result<(), DispatchError> {
    context.plan_version.ok_or(DispatchError::MissingPlan)?;
    context.pr_number.ok_or(DispatchError::MissingPullRequest)?;
    context.head_sha.ok_or(DispatchError::MissingExactHead)?;
    Ok(())
}

fn dispatch(
    effect_id: &str,
    role: WorkerRole,
    context: &DispatchContext,
    policy: &WorkflowPolicy,
) -> Result<WorkflowDispatch, DispatchError> {
    let binding = policy.role(role)?;
    let role_name = role_name(role);
    let review_round = context.remediation_round.saturating_add(1);
    let round = match role {
        WorkerRole::Planner => context.plan_version.map_or(1, |version| version.get() + 1),
        WorkerRole::Builder => context.remediation_round.max(1),
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf | WorkerRole::FinalReviewer => {
            review_round
        }
    };
    let projection_base = format!(
        "{}:{role_name}:round:{round}:revision:{}",
        context.case_id,
        context.state_revision.get()
    );
    let gate_projection_key = format!("{projection_base}:gate");
    let worker_projection_key = format!("{projection_base}:worker");
    let mut body = Map::from_iter([
        ("case_key".into(), json!(context.case_id.to_string())),
        (
            "repository_id".into(),
            json!(context.case_id.repository().get()),
        ),
        ("issue_number".into(), json!(context.case_id.issue().get())),
        (
            "workflow_version".into(),
            json!(context.case_id.workflow().get()),
        ),
        ("state_revision".into(), json!(context.state_revision.get())),
        ("role".into(), json!(role_name)),
        ("remediation_round".into(), json!(context.remediation_round)),
    ]);
    let result_plan_version = if role == WorkerRole::Planner {
        context.plan_version.map_or(1, |version| version.get() + 1)
    } else {
        context
            .plan_version
            .map(PlanVersion::get)
            .unwrap_or_default()
    };
    if result_plan_version > 0 {
        body.insert("plan_version".into(), json!(result_plan_version));
    }
    if let Some(pr_number) = context.pr_number {
        body.insert("pr_number".into(), json!(pr_number.get()));
    }
    if let Some(head) = context.head_sha {
        body.insert("expected_head_sha".into(), json!(head.to_string()));
    }
    if matches!(
        role,
        WorkerRole::ReviewerGeneral | WorkerRole::ReviewerSecperf
    ) {
        body.insert("review_round".into(), json!(review_round));
    }
    if role == WorkerRole::FinalReviewer {
        body.insert("final_review_round".into(), json!(review_round));
    }
    body.insert(
        "execution".into(),
        json!(match binding.execution {
            ExecutionKind::Hermes => "hermes",
            ExecutionKind::Direct => "direct",
        }),
    );
    body.insert("provider".into(), json!(binding.provider));
    body.insert("model".into(), json!(binding.model));
    body.insert(
        "requested_model".into(),
        json!(format!("{}/{}", binding.provider, binding.model)),
    );
    body.insert(
        "skills_repository_commit".into(),
        json!(context.skills_repository_commit.to_string()),
    );

    Ok(WorkflowDispatch {
        role,
        gate: GateCreateSpec {
            board: policy.board.clone(),
            effect_id: format!("{effect_id}:gate"),
            projection_key: gate_projection_key,
            title: format!("Activate {role_name} for {}", context.case_id),
            body: json!({
                "case_key": context.case_id.to_string(),
                "state_revision": context.state_revision.get(),
                "activation_gate": role_name,
            }),
            parent_task_ids: Vec::new(),
        },
        worker_projection_key,
        worker_body: Value::Object(body),
        worker_effect_id: format!("{effect_id}:worker"),
        worker_title: format!("Run {role_name} for {}", context.case_id),
        profile: binding.profile.clone(),
        workspace: policy.workspace.clone(),
        skills: binding.skills.clone(),
        provider: binding.provider.clone(),
        model: binding.model.clone(),
        max_runtime: binding.max_runtime.clone(),
        priority: binding.priority,
        board: policy.board.clone(),
    })
}

fn valid_role_policy(role: &RolePolicy) -> bool {
    let execution_valid = match role.role {
        WorkerRole::Builder | WorkerRole::ReviewerSecperf => {
            role.execution == ExecutionKind::Direct && role.provider == "cursor"
        }
        WorkerRole::Planner | WorkerRole::ReviewerGeneral | WorkerRole::FinalReviewer => {
            role.execution == ExecutionKind::Hermes && role.provider != "cursor"
        }
    };
    let model = role.model.to_ascii_lowercase();
    execution_valid
        && valid_id(&role.profile)
        && valid_id(&role.provider)
        && valid_id(&role.model)
        && !matches!(model.as_str(), "auto" | "default" | "latest")
        && valid_id(&role.max_runtime)
        && role.skills.len() == 2
        && role.skills.iter().all(|skill| valid_id(skill))
        && role.skills.iter().any(|skill| skill == "workflow-contract")
        && role.skills.iter().any(|skill| skill == &role.profile)
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

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_opaque(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace())
}

fn valid_text(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_len
        && value.chars().all(|character| !character.is_control())
}
