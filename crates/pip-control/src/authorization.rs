//! Fresh authorization gate for every active case before task release.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;

use crate::{IntakeSource, RepositoryPolicy, ShadowError};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizationBlock {
    pub case_key: String,
    pub blockers: Vec<String>,
    pub revoked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ActiveAuthorization {
    Authorized { case_count: usize },
    Blocked { cases: Vec<AuthorizationBlock> },
}

impl ActiveAuthorization {
    #[must_use]
    pub const fn is_authorized(&self) -> bool {
        matches!(self, Self::Authorized { .. })
    }
}

#[derive(Debug)]
pub enum AuthorizationError {
    Evidence(ShadowError),
    Store(StoreError),
    Controller(ControllerError),
    InvalidCase,
    Serialization(String),
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::InvalidCase => formatter.write_str("active authorization case is invalid"),
            Self::Serialization(error) => {
                write!(
                    formatter,
                    "authorization evidence serialization failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for AuthorizationError {}

impl From<StoreError> for AuthorizationError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ControllerError> for AuthorizationError {
    fn from(error: ControllerError) -> Self {
        Self::Controller(error)
    }
}

pub fn verify_active_authorization<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &Store,
) -> Result<ActiveAuthorization, AuthorizationError> {
    let mut cases = store
        .status(0)?
        .cases
        .into_iter()
        .filter(|case| {
            case.repository_id == policy.repository.id
                && !matches!(
                    case.state.as_str(),
                    "COMPLETED" | "ABANDONED" | "TAKEN_OVER"
                )
        })
        .collect::<Vec<_>>();
    cases.sort_by_key(|case| (case.issue_number, case.workflow_version));
    let case_count = cases.len();
    let mut blocked = Vec::new();
    for case in cases {
        let evidence = source
            .intake(
                &policy.repository.owner,
                &policy.repository.name,
                case.issue_number,
            )
            .map_err(|error| {
                AuthorizationError::Evidence(ShadowError::Evidence(error.to_string()))
            })?;
        let mut blockers = Vec::new();
        if case.policy_revision != policy.revision {
            blockers.push("POLICY_REVISION_MISMATCH".into());
        }
        if evidence.repository.id != policy.repository.id
            || evidence.repository.full_name != policy.repository.full_name()
            || evidence.repository.default_branch != policy.repository.default_branch
        {
            blockers.push("REPOSITORY_IDENTITY_MISMATCH".into());
        }
        if evidence.issue.number != case.issue_number {
            blockers.push("ISSUE_IDENTITY_MISMATCH".into());
        }
        if !evidence.issue.open {
            blockers.push("ISSUE_CLOSED".into());
        }
        if evidence.issue.is_pull_request {
            blockers.push("ISSUE_BECAME_PULL_REQUEST".into());
        }
        if policy
            .intake
            .excluded_issue_numbers
            .contains(&case.issue_number)
        {
            blockers.push("ISSUE_EXCLUDED".into());
        }
        let label_present = evidence.issue.labels.contains(&policy.intake.label);
        if !label_present {
            blockers.push("REQUIRED_LABEL_MISSING".into());
        }
        let latest = evidence
            .label_events
            .iter()
            .filter(|event| event.label == policy.intake.label)
            .max_by_key(|event| (&event.created_at, event.id));
        match latest {
            None => blockers.push("AUTHORIZATION_EVENT_MISSING".into()),
            Some(event) if !event.labeled => {
                blockers.push("LATEST_AUTHORIZATION_REMOVED".into());
            }
            Some(event) if !policy.intake.trusted_actor_ids.contains(&event.actor_id) => {
                blockers.push("UNTRUSTED_LABEL_ACTOR".into());
            }
            Some(_) => {}
        }
        if !blockers.is_empty() {
            blocked.push(AuthorizationBlock {
                case_key: case.case_key,
                blockers,
                revoked: false,
            });
        }
    }
    if blocked.is_empty() {
        Ok(ActiveAuthorization::Authorized { case_count })
    } else {
        Ok(ActiveAuthorization::Blocked { cases: blocked })
    }
}

pub fn reconcile_active_authorization<S: IntakeSource>(
    source: &S,
    policy: &RepositoryPolicy,
    store: &mut Store,
    observed_at: u64,
) -> Result<ActiveAuthorization, AuthorizationError> {
    let mut authorization = verify_active_authorization(source, policy, store)?;
    let ActiveAuthorization::Blocked { cases } = &mut authorization else {
        return Ok(authorization);
    };
    for blocked in cases {
        if !revocation_is_authoritative(&blocked.blockers) {
            continue;
        }
        let case = store
            .case(&blocked.case_key)?
            .ok_or(AuthorizationError::InvalidCase)?;
        if case.policy_revision != policy.revision {
            continue;
        }
        let evidence = source
            .intake(
                &policy.repository.owner,
                &policy.repository.name,
                case.issue_number,
            )
            .map_err(|error| {
                AuthorizationError::Evidence(ShadowError::Evidence(error.to_string()))
            })?;
        let payload = serde_json::to_value(&evidence)
            .map_err(|error| AuthorizationError::Serialization(error.to_string()))?;
        let command = revocation_command(
            &case,
            policy,
            observed_at,
            blocked.blockers.clone(),
            EvidenceInput {
                evidence_id: format!(
                    "evidence-authorization-revoked-repo{}-issue{}-workflow{}-revision{}",
                    case.repository_id,
                    case.issue_number,
                    case.workflow_version,
                    case.state_revision
                ),
                kind: "GITHUB_AUTHORIZATION".into(),
                source: format!("github-issue-{}", case.issue_number),
                payload,
            },
        )?;
        LedgerController::apply(store, &policy.case_policy(), &command)?;
        blocked.revoked = true;
    }
    Ok(authorization)
}

fn revocation_is_authoritative(blockers: &[String]) -> bool {
    let structural = [
        "POLICY_REVISION_MISMATCH",
        "REPOSITORY_IDENTITY_MISMATCH",
        "ISSUE_IDENTITY_MISMATCH",
        "ISSUE_BECAME_PULL_REQUEST",
        "AUTHORIZATION_EVENT_MISSING",
    ];
    !blockers
        .iter()
        .any(|blocker| structural.contains(&blocker.as_str()))
        && blockers.iter().any(|blocker| {
            matches!(
                blocker.as_str(),
                "ISSUE_CLOSED"
                    | "ISSUE_EXCLUDED"
                    | "REQUIRED_LABEL_MISSING"
                    | "LATEST_AUTHORIZATION_REMOVED"
                    | "UNTRUSTED_LABEL_ACTOR"
            )
        })
}

fn revocation_command(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    observed_at: u64,
    blockers: Vec<String>,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, AuthorizationError> {
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(AuthorizationError::InvalidCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(AuthorizationError::InvalidCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(AuthorizationError::InvalidCase)?,
        ),
    );
    if case_id.to_string() != case.case_key || case.policy_revision != policy.revision {
        return Err(AuthorizationError::InvalidCase);
    }
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-authorization-revoked-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| AuthorizationError::InvalidCase)?,
        observed_at: ObservedAt::new(observed_at),
        expected_state: CaseState::from_str(&case.state)
            .map_err(|_| AuthorizationError::InvalidCase)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(AuthorizationError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(AuthorizationError::InvalidCase)?,
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
            .map_err(|_| AuthorizationError::InvalidCase)?,
        event: Event::AuthorizationRemoved,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: serde_json::json!({"blockers": blockers}),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}
