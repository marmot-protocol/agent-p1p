//! Controller-owned publication of immutable planner results.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_contracts::{PlannerOutcome, PlannerResult, WorkerResult};
use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_github::{CommentSpec, GitHubError, GitHubWriter, MutationResult, MutationTransport};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::RepositoryPolicy;

const PUBLISH_EFFECT: &str = "PUBLISH_PLAN";

pub trait PlanWriter {
    fn ensure_plan_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError>;
}

impl<T: MutationTransport> PlanWriter for GitHubWriter<T> {
    fn ensure_plan_comment(&self, spec: &CommentSpec) -> Result<MutationResult, GitHubError> {
        self.ensure_issue_comment(spec)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PlanPublicationCycle {
    Idle,
    AuthorizationBlocked,
    Published {
        case_key: String,
        plan_version: u32,
        comment_id: u64,
    },
}

#[derive(Debug)]
pub enum PlanPublicationError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    MissingAutomationActor,
    InvalidCase,
    InvalidPlanJoin,
    UnexpectedMutationResult,
    Serialization(String),
}

impl fmt::Display for PlanPublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::MissingAutomationActor => {
                formatter.write_str("plan publication requires an automation actor")
            }
            Self::InvalidCase => formatter.write_str("plan publication case is invalid"),
            Self::InvalidPlanJoin => {
                formatter.write_str("plan publication requires one next-version planner result")
            }
            Self::UnexpectedMutationResult => {
                formatter.write_str("plan mutation returned a non-comment result")
            }
            Self::Serialization(error) => {
                write!(formatter, "plan publication serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for PlanPublicationError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for PlanPublicationError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(GitHubError, GitHub);
error_from!(StoreError, Store);
error_from!(ControllerError, Controller);

#[allow(clippy::too_many_arguments)]
pub fn publish_plan_once<W: PlanWriter>(
    writer: &W,
    policy: &RepositoryPolicy,
    store: &mut Store,
    now: u64,
    owner: &str,
    lease_seconds: u64,
    authorization_valid: bool,
) -> Result<PlanPublicationCycle, PlanPublicationError> {
    if !authorization_valid {
        return Ok(PlanPublicationCycle::AuthorizationBlocked);
    }
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(PlanPublicationError::MissingAutomationActor)?;
    let Some(claimed) =
        store.claim_effect_matching(owner, now, lease_seconds, &[PUBLISH_EFFECT])?
    else {
        return Ok(PlanPublicationCycle::Idle);
    };
    let case = store
        .case(&claimed.case_key)?
        .ok_or(PlanPublicationError::InvalidCase)?;
    if case.state != "PLANNING"
        || case.repository_id != policy.repository.id
        || case.policy_revision != policy.revision
        || case.state_revision != claimed.state_revision
    {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(PlanPublicationError::InvalidCase);
    }
    let plan = match next_plan(store, &case) {
        Ok(plan) => plan,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    let body = match render_plan(&plan) {
        Ok(body) => body,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    let mutation = writer.ensure_plan_comment(&CommentSpec {
        owner: policy.repository.owner.clone(),
        repository: policy.repository.name.clone(),
        issue_number: case.issue_number,
        effect_id: claimed.effect_id.clone(),
        expected_actor_id: expected_actor,
        body: body.clone(),
    });
    let comment_id = match mutation {
        Ok(MutationResult::Created(id) | MutationResult::Existing(id)) => id,
        Ok(MutationResult::Updated(_) | MutationResult::Merged(_)) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(PlanPublicationError::UnexpectedMutationResult);
        }
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error.into());
        }
    };
    let evidence = EvidenceInput {
        evidence_id: format!("evidence-plan-{}", claimed.effect_id),
        kind: "GITHUB_PLAN_PUBLICATION".into(),
        source: format!("github-issue-{}", case.issue_number),
        payload: json!({
            "actor_id": expected_actor,
            "comment_id": comment_id,
            "comment_body_sha256": hex_digest(&Sha256::digest(body.as_bytes())),
            "plan_version": plan.plan_version,
            "task_id": plan.common.task_id,
        }),
    };
    let command = match published_command(&case, &plan, now, evidence) {
        Ok(command) => command,
        Err(error) => {
            store.release_effect(&claimed.effect_id, owner)?;
            return Err(error);
        }
    };
    if let Err(error) = LedgerController::apply(store, &policy.case_policy(), &command) {
        store.release_effect(&claimed.effect_id, owner)?;
        return Err(error.into());
    }
    Ok(PlanPublicationCycle::Published {
        case_key: case.case_key,
        plan_version: plan.plan_version,
        comment_id,
    })
}

fn next_plan(store: &Store, case: &StoredCase) -> Result<PlannerResult, PlanPublicationError> {
    let expected = case.plan_version.saturating_add(1);
    let mut matching = Vec::new();
    for run in store.runs_for_case(&case.case_key)? {
        let result: WorkerResult = serde_json::from_value(run.payload)
            .map_err(|error| PlanPublicationError::Serialization(error.to_string()))?;
        if let WorkerResult::Planner(plan) = result
            && plan.plan_version == expected
        {
            matching.push(plan);
        }
    }
    match matching.as_slice() {
        [plan] => Ok(plan.clone()),
        _ => Err(PlanPublicationError::InvalidPlanJoin),
    }
}

fn render_plan(plan: &PlannerResult) -> Result<String, PlanPublicationError> {
    let dependencies = serde_json::to_string_pretty(&plan.dependencies)
        .map_err(|error| PlanPublicationError::Serialization(error.to_string()))?;
    let decisions = serde_json::to_string_pretty(&plan.open_decisions)
        .map_err(|error| PlanPublicationError::Serialization(error.to_string()))?;
    let sensitive = serde_json::to_string_pretty(&plan.sensitive_scope)
        .map_err(|error| PlanPublicationError::Serialization(error.to_string()))?;
    let binding = if plan.outcome == PlannerOutcome::Proceed {
        let value = json!({
            "authorized_scope": plan.authorized_scope,
            "dependencies": plan.dependencies,
            "open_decisions": plan.open_decisions,
            "outcome": "PROCEED",
            "plan_version": plan.plan_version,
            "sensitive_scope": plan.sensitive_scope,
            "task_id": plan.common.task_id,
        });
        format!(
            "\n\nPip execution binding: {}",
            serde_json::to_string(&value)
                .map_err(|error| PlanPublicationError::Serialization(error.to_string()))?
        )
    } else {
        String::new()
    };
    Ok(format!(
        "## Pip plan v{}: {}\n\nPlanned base: `{}`\n\n### Root cause\n\n{}\n\n### Authorized scope\n\n{}\n\n### Sensitive scope\n\n```json\n{sensitive}\n```\n\n### Dependencies\n\n```json\n{dependencies}\n```\n\n### Open decisions\n\n```json\n{decisions}\n```\n\nPlan artifact: `{}`\n\nPip planner task: `{}`{binding}",
        plan.plan_version,
        outcome_name(plan.outcome),
        plan.planned_base_sha,
        plan.root_cause,
        plan.authorized_scope,
        plan.plan_artifact,
        plan.common.task_id,
    ))
}

fn published_command(
    case: &StoredCase,
    plan: &PlannerResult,
    now: u64,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, PlanPublicationError> {
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(PlanPublicationError::InvalidCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(PlanPublicationError::InvalidCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(PlanPublicationError::InvalidCase)?,
        ),
    );
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-plan-published-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| PlanPublicationError::InvalidCase)?,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::Planning,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(PlanPublicationError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(PlanPublicationError::InvalidCase)?,
        ),
        remediation_round: case.remediation_round,
        plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
        pr_number: case
            .pr_number
            .and_then(NonZeroU64::new)
            .map(PullRequestNumber::new),
        head_sha: case
            .head_sha
            .as_deref()
            .map(GitSha::from_str)
            .transpose()
            .map_err(|_| PlanPublicationError::InvalidCase)?,
        event: planner_event(plan.outcome),
        accepted_plan_version: (plan.outcome == PlannerOutcome::Proceed)
            .then(|| NonZeroU32::new(plan.plan_version).map(PlanVersion::new))
            .flatten(),
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({
            "planner_result": plan,
            "publication": evidence.payload,
        }),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}

fn planner_event(outcome: PlannerOutcome) -> Event {
    match outcome {
        PlannerOutcome::Proceed => Event::Proceed,
        PlannerOutcome::AlreadyFixed => Event::AlreadyFixed,
        PlannerOutcome::NotReproducible => Event::NotReproducible,
        PlannerOutcome::Duplicate => Event::Duplicate,
        PlannerOutcome::RootCauseDifferentScope => Event::RootCauseDifferentScope,
        PlannerOutcome::CrossRepoDependency => Event::CrossRepoDependency,
        PlannerOutcome::WaitingForIssueCreator => Event::WaitingForIssueCreator,
        PlannerOutcome::NeedsHumanScopeDecision => Event::NeedsHumanScopeDecision,
        PlannerOutcome::Abandon => Event::Abandon,
        PlannerOutcome::Blocked => Event::Blocked,
        PlannerOutcome::BlockedUnexpectedModel => Event::BlockedUnexpectedModel,
    }
}

fn outcome_name(outcome: PlannerOutcome) -> &'static str {
    match outcome {
        PlannerOutcome::Proceed => "PROCEED",
        PlannerOutcome::AlreadyFixed => "ALREADY_FIXED",
        PlannerOutcome::NotReproducible => "NOT_REPRODUCIBLE",
        PlannerOutcome::Duplicate => "DUPLICATE",
        PlannerOutcome::RootCauseDifferentScope => "ROOT_CAUSE_DIFFERENT_SCOPE",
        PlannerOutcome::CrossRepoDependency => "CROSS_REPO_DEPENDENCY",
        PlannerOutcome::WaitingForIssueCreator => "WAITING_FOR_ISSUE_CREATOR",
        PlannerOutcome::NeedsHumanScopeDecision => "NEEDS_HUMAN_SCOPE_DECISION",
        PlannerOutcome::Abandon => "ABANDON",
        PlannerOutcome::Blocked => "BLOCKED",
        PlannerOutcome::BlockedUnexpectedModel => "BLOCKED_UNEXPECTED_MODEL",
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
