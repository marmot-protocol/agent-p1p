//! Fresh pull-request ownership reconciliation and durable human takeover.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_github::{GitHubError, PullRequestSnapshot};
use pip_store::{EvidenceInput, Store, StoreError, StoredCase};
use serde::Serialize;

use crate::{PullRequestSource, RepositoryPolicy};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TakeoverCycle {
    Idle,
    Owned { case_count: usize },
    Transitioned { cases: Vec<String> },
}

#[derive(Debug)]
pub enum TakeoverError {
    GitHub(GitHubError),
    Store(StoreError),
    Controller(ControllerError),
    MissingAutomationActor,
    InvalidCase,
    RepositoryDrift,
    Serialization(String),
}

impl fmt::Display for TakeoverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::MissingAutomationActor => {
                formatter.write_str("active takeover checks require a GitHub automation actor")
            }
            Self::InvalidCase => formatter.write_str("takeover case binding is invalid"),
            Self::RepositoryDrift => {
                formatter.write_str("pull request repository, number, or base drifted")
            }
            Self::Serialization(error) => {
                write!(formatter, "takeover evidence serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for TakeoverError {}

macro_rules! error_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for TakeoverError {
            fn from(error: $type) -> Self {
                Self::$variant(error)
            }
        }
    };
}

error_from!(GitHubError, GitHub);
error_from!(StoreError, Store);
error_from!(ControllerError, Controller);

pub fn reconcile_takeover_once<'a, S: PullRequestSource>(
    source: &S,
    scope: impl Into<crate::RepositoryScope<'a>>,
    store: &mut Store,
    observed_at: u64,
) -> Result<TakeoverCycle, TakeoverError> {
    let scope = scope.into();
    let policy = scope.policy;
    let expected_actor = policy
        .github
        .automation_actor_id
        .ok_or(TakeoverError::MissingAutomationActor)?;
    let mut cases = store
        .status(observed_at)?
        .cases
        .into_iter()
        .filter(|case| {
            scope.matches(case)
                && case.pr_number.is_some()
                && !matches!(case.state.as_str(), "COMPLETED" | "ABANDONED")
        })
        .collect::<Vec<_>>();
    cases.sort_by_key(|case| (case.issue_number, case.workflow_version));
    if cases.is_empty() {
        return Ok(TakeoverCycle::Idle);
    }
    let mut case_count = 0;
    let mut transitioned = Vec::new();
    for case in cases {
        let historical_ready = if case.state == "TAKEN_OVER" {
            let Some(ready) = legacy_merged_ready_case(store, &case, policy, expected_actor)?
            else {
                continue;
            };
            Some(ready)
        } else {
            None
        };
        case_count += 1;
        if case.policy_revision != policy.revision {
            return Err(TakeoverError::InvalidCase);
        }
        let pr_number = case.pr_number.ok_or(TakeoverError::InvalidCase)?;
        let evidence = source.pull_request(
            &policy.repository.owner,
            &policy.repository.name,
            policy.repository.id,
            pr_number,
        )?;
        validate_repository_identity(&case, policy, &evidence.pull_request)?;
        let binding = historical_ready
            .as_ref()
            .map(|(ready, _)| ready)
            .unwrap_or(&case);
        let blockers = takeover_blockers(binding, policy, expected_actor, &evidence.pull_request)?;
        let completed = binding.state == "SHADOW_READY"
            && blockers == ["PR_DISPOSITION_CHANGED"]
            && confirmed_merge(&evidence.pull_request)
            && historical_ready.as_ref().is_none_or(|(_, merge_sha)| {
                evidence.pull_request.merge_commit_sha.as_ref() == Some(merge_sha)
            });
        // A correction cannot reinterpret a later human change as Pip success.
        if historical_ready.is_some() && !completed {
            continue;
        }
        if blockers.is_empty() {
            continue;
        }
        let payload = serde_json::to_value(&evidence)
            .map_err(|error| TakeoverError::Serialization(error.to_string()))?;
        let mut command = takeover_command(
            &case,
            policy,
            observed_at,
            blockers,
            EvidenceInput {
                evidence_id: format!(
                    "evidence-takeover-repo{}-issue{}-workflow{}-revision{}",
                    case.repository_id,
                    case.issue_number,
                    case.workflow_version,
                    case.state_revision
                ),
                kind: if completed {
                    "GITHUB_HUMAN_MERGE"
                } else {
                    "GITHUB_TAKEOVER"
                }
                .into(),
                source: format!("github-pr-{pr_number}"),
                payload,
            },
        )?;
        if completed {
            command.event = Event::HumanMerged;
            command.event_payload = serde_json::json!({
                "pull_request_number": pr_number,
                "head_sha": evidence.pull_request.head_sha,
                "merge_commit_sha": evidence.pull_request.merge_commit_sha,
                "corrects_takeover_revision": historical_ready.as_ref().map(|_| case.state_revision),
            });
        }
        LedgerController::apply(store, &policy.case_policy(), &command)?;
        transitioned.push(case.case_key);
    }
    if case_count == 0 {
        Ok(TakeoverCycle::Idle)
    } else if transitioned.is_empty() {
        Ok(TakeoverCycle::Owned { case_count })
    } else {
        Ok(TakeoverCycle::Transitioned {
            cases: transitioned,
        })
    }
}

fn confirmed_merge(pull: &PullRequestSnapshot) -> bool {
    pull.merged
        && !pull.open
        && !pull.draft
        && pull
            .merge_commit_sha
            .as_deref()
            .is_some_and(|sha| GitSha::from_str(sha).is_ok())
}

/// Correct only the former ready -> takeover-on-merge bug. No event or evidence
/// is rewritten, and genuine takeovers are not polled or revived.
fn legacy_merged_ready_case(
    store: &Store,
    case: &StoredCase,
    policy: &RepositoryPolicy,
    expected_actor: u64,
) -> Result<Option<(StoredCase, String)>, TakeoverError> {
    let Some(revision) = case.state_revision.checked_sub(1) else {
        return Ok(None);
    };
    let Some(prior) = store.case_at_revision(&case.case_key, revision)? else {
        return Ok(None);
    };
    if prior.state != "SHADOW_READY"
        || prior.head_sha != case.head_sha
        || prior.pr_number != case.pr_number
        || prior.policy_revision != case.policy_revision
    {
        return Ok(None);
    }
    let history = store.immutable_history_for_case(&case.case_key)?;
    let Some(event) = history.events.last() else {
        return Ok(None);
    };
    if event.state_revision != case.state_revision
        || event.event_type != "HUMAN_TOOK_OVER"
        || event.payload["blockers"] != serde_json::json!(["PR_DISPOSITION_CHANGED"])
        || !crate::draft_pr::payload_matches(&event.payload, &event.payload_sha256)
    {
        return Ok(None);
    }
    let evidence_id = format!(
        "evidence-takeover-repo{}-issue{}-workflow{}-revision{}",
        case.repository_id, case.issue_number, case.workflow_version, revision
    );
    let Some(record) = history
        .evidence
        .iter()
        .find(|record| record.evidence_id == evidence_id && record.kind == "GITHUB_TAKEOVER")
    else {
        return Ok(None);
    };
    if !crate::draft_pr::payload_matches(&record.payload, &record.payload_sha256) {
        return Ok(None);
    }
    let Ok(pull) =
        serde_json::from_value::<PullRequestSnapshot>(record.payload["pull_request"].clone())
    else {
        return Ok(None);
    };
    let valid = confirmed_merge(&pull)
        && validate_repository_identity(&prior, policy, &pull).is_ok()
        && takeover_blockers(&prior, policy, expected_actor, &pull)? == ["PR_DISPOSITION_CHANGED"];
    Ok(valid.then(|| {
        (
            prior,
            pull.merge_commit_sha.expect("confirmed merge has a commit"),
        )
    }))
}

fn validate_repository_identity(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    pull: &PullRequestSnapshot,
) -> Result<(), TakeoverError> {
    if pull.number != case.pr_number.ok_or(TakeoverError::InvalidCase)?
        || pull.head_repository_id != policy.repository.id
        || pull.head_repository != policy.repository.full_name()
        || pull.base_branch != policy.repository.default_branch
    {
        return Err(TakeoverError::RepositoryDrift);
    }
    Ok(())
}

fn takeover_blockers(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    expected_actor: u64,
    pull: &PullRequestSnapshot,
) -> Result<Vec<String>, TakeoverError> {
    let expected_branch = format!(
        "{}repo-{}/issue-{}/workflow-{}",
        policy.branch_prefix, case.repository_id, case.issue_number, case.workflow_version
    );
    let mut blockers = Vec::new();
    if pull.author_id != expected_actor {
        blockers.push("FOREIGN_PR_AUTHOR".into());
    }
    if pull.head_branch != expected_branch {
        blockers.push("FOREIGN_HEAD_BRANCH".into());
    }
    if !pull.open || pull.merged {
        blockers.push("PR_DISPOSITION_CHANGED".into());
    }
    // The controller promotes a validated shadow-ready PR before notifying its
    // human. A retry may observe that promotion before its evidence is stored.
    if !pull.draft && case.state != "SHADOW_READY" {
        blockers.push("PR_LEFT_DRAFT_STATE".into());
    }
    let protected_head = !matches!(case.state.as_str(), "BUILDING" | "REMEDIATING");
    if protected_head
        && pull.head_sha != case.head_sha.as_deref().ok_or(TakeoverError::InvalidCase)?
    {
        blockers.push("FOREIGN_HEAD_COMMIT".into());
    }
    Ok(blockers)
}

fn takeover_command(
    case: &StoredCase,
    policy: &RepositoryPolicy,
    observed_at: u64,
    blockers: Vec<String>,
    evidence: EvidenceInput,
) -> Result<WorkflowCommand, TakeoverError> {
    let case_id = CaseId::new(
        RepositoryId::new(NonZeroU64::new(case.repository_id).ok_or(TakeoverError::InvalidCase)?),
        IssueNumber::new(NonZeroU64::new(case.issue_number).ok_or(TakeoverError::InvalidCase)?),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(TakeoverError::InvalidCase)?,
        ),
    );
    if case_id.to_string() != case.case_key || case.policy_revision != policy.revision {
        return Err(TakeoverError::InvalidCase);
    }
    Ok(WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-takeover-repo{}-issue{}-workflow{}-revision{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| TakeoverError::InvalidCase)?,
        observed_at: ObservedAt::new(observed_at),
        expected_state: CaseState::from_str(&case.state).map_err(|_| TakeoverError::InvalidCase)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(TakeoverError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(TakeoverError::InvalidCase)?,
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
            .map_err(|_| TakeoverError::InvalidCase)?,
        event: Event::HumanTookOver,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: serde_json::json!({"blockers": blockers}),
        run: None,
        evidence: vec![evidence],
        findings: Vec::new(),
    })
}
