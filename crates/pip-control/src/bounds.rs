//! Deterministic operational limits layered over the pure workflow state machine.

use std::collections::BTreeMap;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::{ControllerError, LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::{Store, StoreError, StoredCase};
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::RepositoryPolicy;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationalBound {
    ElapsedTime,
    ProviderFailures,
    RepeatedFindingFingerprint,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum OperationalBoundsCycle {
    Idle,
    Escalated {
        case_key: String,
        bound: OperationalBound,
        observed: u64,
        limit: u64,
    },
}

pub(crate) struct BoundObservation {
    pub bound: OperationalBound,
    pub observed: u64,
    pub limit: u64,
    pub details: Value,
}

#[derive(Debug)]
pub enum OperationalBoundsError {
    Store(StoreError),
    Controller(ControllerError),
    InvalidCase,
}

impl fmt::Display for OperationalBoundsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Controller(error) => error.fmt(formatter),
            Self::InvalidCase => formatter.write_str("operational bound found an invalid case"),
        }
    }
}

impl std::error::Error for OperationalBoundsError {}

impl From<StoreError> for OperationalBoundsError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ControllerError> for OperationalBoundsError {
    fn from(error: ControllerError) -> Self {
        Self::Controller(error)
    }
}

pub fn enforce_operational_bounds(
    store: &mut Store,
    policy: &RepositoryPolicy,
    now: u64,
) -> Result<OperationalBoundsCycle, OperationalBoundsError> {
    let mut cases = store.status(now)?.cases;
    cases.sort_by(|left, right| left.case_key.cmp(&right.case_key));
    for case in cases {
        if case.repository_id != policy.repository.id || !automated_state(&case.state) {
            continue;
        }
        let created_at = store
            .case_created_at(&case.case_key)?
            .ok_or(OperationalBoundsError::InvalidCase)?;
        let elapsed = now.saturating_sub(created_at);
        let elapsed_limit = policy.max_case_elapsed_seconds;
        if elapsed >= elapsed_limit {
            return escalate(
                store,
                policy,
                &case,
                now,
                BoundObservation {
                    bound: OperationalBound::ElapsedTime,
                    observed: elapsed,
                    limit: elapsed_limit,
                    details: json!({"source": "controller"}),
                },
            );
        }
        let provider_failures = store.failed_direct_attempt_count_for_case(&case.case_key)?;
        let provider_limit = u64::from(policy.max_provider_failures);
        if provider_failures >= provider_limit {
            return escalate(
                store,
                policy,
                &case,
                now,
                BoundObservation {
                    bound: OperationalBound::ProviderFailures,
                    observed: provider_failures,
                    limit: provider_limit,
                    details: json!({"source": "direct-worker"}),
                },
            );
        }
        let repeated = repeated_finding_count(store, &case.case_key)?;
        let repeated_limit = u64::from(policy.max_repeated_finding_fingerprint);
        if repeated >= repeated_limit {
            return escalate(
                store,
                policy,
                &case,
                now,
                BoundObservation {
                    bound: OperationalBound::RepeatedFindingFingerprint,
                    observed: repeated,
                    limit: repeated_limit,
                    details: json!({"source": "finding-ledger"}),
                },
            );
        }
    }
    Ok(OperationalBoundsCycle::Idle)
}

fn automated_state(state: &str) -> bool {
    matches!(
        state,
        "PLANNING"
            | "READY_TO_BUILD"
            | "BUILDING"
            | "WAITING_CI"
            | "REVIEWING"
            | "REMEDIATING"
            | "FINAL_REVIEW"
            | "READY_TO_MERGE"
            | "MERGING"
    )
}

fn repeated_finding_count(store: &Store, case_key: &str) -> Result<u64, StoreError> {
    let mut counts = BTreeMap::<String, u64>::new();
    for finding in store.immutable_history_for_case(case_key)?.findings {
        let Some(payload) = finding.payload.as_object() else {
            continue;
        };
        let fingerprint_source = json!({
            "origin_role": finding.origin_role,
            "defect": payload.get("defect"),
            "consequence": payload.get("consequence"),
            "corrective_direction": payload.get("corrective_direction"),
        });
        let digest = Sha256::digest(serde_json::to_vec(&fingerprint_source)?);
        let fingerprint = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        *counts.entry(fingerprint).or_default() += 1;
    }
    Ok(counts.values().copied().max().unwrap_or(0))
}

fn escalate(
    store: &mut Store,
    policy: &RepositoryPolicy,
    case: &StoredCase,
    now: u64,
    observation: BoundObservation,
) -> Result<OperationalBoundsCycle, OperationalBoundsError> {
    let case_id = CaseId::new(
        RepositoryId::new(
            NonZeroU64::new(case.repository_id).ok_or(OperationalBoundsError::InvalidCase)?,
        ),
        IssueNumber::new(
            NonZeroU64::new(case.issue_number).ok_or(OperationalBoundsError::InvalidCase)?,
        ),
        WorkflowVersion::new(
            NonZeroU32::new(case.workflow_version).ok_or(OperationalBoundsError::InvalidCase)?,
        ),
    );
    let command = WorkflowCommand {
        case_id,
        event_id: EventId::from_str(&format!(
            "event-operational-bound-{}-{}-{}-{}",
            case.repository_id, case.issue_number, case.workflow_version, case.state_revision
        ))
        .map_err(|_| OperationalBoundsError::InvalidCase)?,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::from_str(&case.state)
            .map_err(|_| OperationalBoundsError::InvalidCase)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(case.state_revision).ok_or(OperationalBoundsError::InvalidCase)?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or(OperationalBoundsError::InvalidCase)?,
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
            .map_err(|_| OperationalBoundsError::InvalidCase)?,
        event: Event::OperationalBoundReached,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: json!({
            "bound": observation.bound,
            "observed": observation.observed,
            "limit": observation.limit,
            "details": observation.details,
        }),
        run: None,
        evidence: Vec::new(),
        findings: Vec::new(),
    };
    LedgerController::apply(store, &policy.case_policy(), &command)?;
    Ok(OperationalBoundsCycle::Escalated {
        case_key: case.case_key.clone(),
        bound: observation.bound,
        observed: observation.observed,
        limit: observation.limit,
    })
}

pub(crate) fn escalate_case_for_bound(
    store: &mut Store,
    policy: &RepositoryPolicy,
    case_key: &str,
    now: u64,
    observation: BoundObservation,
) -> Result<OperationalBoundsCycle, OperationalBoundsError> {
    let case = store
        .case(case_key)?
        .ok_or(OperationalBoundsError::InvalidCase)?;
    if case.repository_id != policy.repository.id || !automated_state(&case.state) {
        return Err(OperationalBoundsError::InvalidCase);
    }
    escalate(store, policy, &case, now, observation)
}
