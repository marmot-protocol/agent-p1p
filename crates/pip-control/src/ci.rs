//! Independent exact-head GitHub CI reconciliation.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_github::{
    CiVerdict, GitHubError, GitHubReader, PullRequestEvidence, ReadTransport, evaluate_ci,
};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::json;

use crate::RepositoryPolicy;

pub trait PullRequestSource {
    fn pull_request(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError>;
}

impl<T: ReadTransport> PullRequestSource for GitHubReader<T> {
    fn pull_request(
        &self,
        owner: &str,
        repository: &str,
        repository_id: u64,
        pull_request_number: u64,
    ) -> Result<PullRequestEvidence, GitHubError> {
        self.read_pull_request(owner, repository, repository_id, pull_request_number)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CiCycle {
    Idle,
    Pending {
        case_key: String,
        blockers: Vec<String>,
    },
    Transitioned {
        case_key: String,
        verdict: String,
    },
}

#[derive(Debug)]
pub enum CiCycleError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    InvalidCase,
    RepositoryDrift,
    PullRequestDrift,
    Serialization(String),
}

impl fmt::Display for CiCycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::InvalidCase => formatter.write_str("waiting-CI case has an invalid binding"),
            Self::RepositoryDrift => {
                formatter.write_str("pull request repository or base differs from policy")
            }
            Self::PullRequestDrift => {
                formatter.write_str("pull request number, branch, or head differs from the ledger")
            }
            Self::Serialization(error) => {
                write!(formatter, "CI evidence serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for CiCycleError {}

impl From<GitHubError> for CiCycleError {
    fn from(error: GitHubError) -> Self {
        Self::GitHub(error)
    }
}

impl From<StoreError> for CiCycleError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ControllerError> for CiCycleError {
    fn from(error: ControllerError) -> Self {
        Self::Controller(error)
    }
}

pub fn reconcile_ci_once<'a, S: PullRequestSource>(
    source: &S,
    scope: impl Into<crate::RepositoryScope<'a>>,
    store: &mut Store,
    observed_at: u64,
) -> Result<CiCycle, CiCycleError> {
    let scope = scope.into();
    let policy = scope.policy;
    let status = store.status(observed_at)?;
    let Some(case) = status
        .cases
        .into_iter()
        .find(|case| scope.matches(case) && case.state == "WAITING_CI")
    else {
        return Ok(CiCycle::Idle);
    };
    let pr_number = case.pr_number.ok_or(CiCycleError::InvalidCase)?;
    let expected_head = case.head_sha.as_deref().ok_or(CiCycleError::InvalidCase)?;
    let evidence = source.pull_request(
        &policy.repository.owner,
        &policy.repository.name,
        policy.repository.id,
        pr_number,
    )?;
    if evidence.pull_request.head_repository_id != policy.repository.id
        || evidence.pull_request.head_repository != policy.repository.full_name()
        || evidence.pull_request.base_branch != policy.repository.default_branch
    {
        return Err(CiCycleError::RepositoryDrift);
    }
    if evidence.pull_request.number != pr_number
        || evidence.pull_request.head_sha != expected_head
        || !evidence
            .pull_request
            .head_branch
            .starts_with(&policy.branch_prefix)
    {
        return Err(CiCycleError::PullRequestDrift);
    }
    let evaluation = evaluate_ci(&evidence, expected_head, &policy.required_ci_contexts);
    if evaluation.verdict == CiVerdict::Pending {
        return Ok(CiCycle::Pending {
            case_key: case.case_key,
            blockers: evaluation.blockers,
        });
    }
    let verdict = match evaluation.verdict {
        CiVerdict::Accepted => "ACCEPTED",
        CiVerdict::Failed => "FAILED",
        CiVerdict::Pending => unreachable!("returned above"),
    };
    let event = if evaluation.verdict == CiVerdict::Accepted {
        Event::CiAccepted
    } else {
        Event::CiFailed
    };
    let evidence_payload = serde_json::to_value(&evidence)
        .map_err(|error| CiCycleError::Serialization(error.to_string()))?;
    let event_payload = json!({
        "verdict": verdict,
        "head_sha": expected_head,
        "blockers": evaluation.blockers,
        "pull_request_id": evidence.pull_request.id,
        "pull_request_number": evidence.pull_request.number,
    });
    let command = workflow(
        &case,
        policy,
        observed_at,
        event,
        event_payload,
        EvidenceInput {
            evidence_id: format!(
                "evidence-ci-repo{}-issue{}-workflow{}-revision{}-head{}",
                case.repository_id,
                case.issue_number,
                case.workflow_version,
                case.state_revision,
                expected_head
            ),
            kind: "GITHUB_CI".into(),
            source: format!("github-pr-{pr_number}"),
            payload: evidence_payload,
        },
    )?;
    LedgerController::apply(store, &policy.case_policy(), &command)?;
    Ok(CiCycle::Transitioned {
        case_key: case.case_key,
        verdict: verdict.into(),
    })
}

fn workflow(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    observed_at: u64,
    event: Event,
    event_payload: serde_json::Value,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, CiCycleError> {
    let case_id = CaseId::new(
        RepositoryId::new(NonZeroU64::new(case.repository_id).ok_or(CiCycleError::InvalidCase)?),
        IssueNumber::new(NonZeroU64::new(case.issue_number).ok_or(CiCycleError::InvalidCase)?),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(CiCycleError::InvalidCase)?,
        ),
    );
    if case_id.to_string() != case.case_key || case.policy_revision != policy.revision {
        return Err(CiCycleError::InvalidCase);
    }
    let verdict = if event == Event::CiAccepted {
        "accepted"
    } else {
        "failed"
    };
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-ci-repo{}-issue{}-workflow{}-revision{}-{verdict}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| CiCycleError::InvalidCase)?,
        observed_at: ObservedAt::new(observed_at),
        expected_state: CaseState::WaitingCi,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(CiCycleError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(CiCycleError::InvalidCase)?,
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
            .map_err(|_| CiCycleError::InvalidCase)?,
        event,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload,
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}
