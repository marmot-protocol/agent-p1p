//! Shared validation for offline, root-authorized recovery commands.
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::WorkflowCommand;
use pip_core::{
    CaseId, CaseState, Event, EventId, GitSha, IssueNumber, ObservedAt, PlanVersion,
    PolicyRevision, PullRequestNumber, RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::Store;

use crate::{RepositoryPolicy, load_repository_policy};

pub(crate) fn recovery_context(
    store: &Store,
    paused: &RepositoryPolicy,
    case_key: &str,
    operator_uid: u32,
) -> Result<(pip_store::StoredCase, RepositoryPolicy), String> {
    if operator_uid != 0 {
        return Err("work retry requires root authorization".into());
    }
    if paused.intake.enabled || !paused.intake.paused || paused.dispatch_enabled {
        return Err("work retry requires an inert installed policy".into());
    }
    let case = store
        .case(case_key)
        .map_err(error)?
        .ok_or("case is missing")?;
    let policy_value = store
        .accepted_policy(case.repository_id, case.policy_revision)
        .map_err(error)?;
    let accepted = load_repository_policy(&serde_json::to_vec(&policy_value).map_err(error)?)
        .map_err(error)?;
    if paused.repository != accepted.repository
        || paused.workflow_version != accepted.workflow_version
        || paused.checkout != accepted.checkout
        || paused.workspace != accepted.workspace
        || serde_json::to_value(&paused.roles).map_err(error)?
            != serde_json::to_value(&accepted.roles).map_err(error)?
    {
        return Err("installed and accepted repository/runtime bindings differ".into());
    }
    Ok((case, accepted))
}

pub(crate) fn recovery_command(
    case: &pip_store::StoredCase,
    expected_revision: u64,
    event_id: EventId,
    event: Event,
    payload: serde_json::Value,
    now: u64,
) -> Result<WorkflowCommand, String> {
    Ok(WorkflowCommand {
        case_id: CaseId::new(
            RepositoryId::new(NonZeroU64::new(case.repository_id).ok_or("invalid repository")?),
            IssueNumber::new(NonZeroU64::new(case.issue_number).ok_or("invalid issue")?),
            WorkflowVersion::new(NonZeroU32::new(case.workflow_version).ok_or("invalid workflow")?),
        ),
        event_id,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::from_str(&case.state).map_err(error)?,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(expected_revision).ok_or("invalid state revision")?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or("invalid policy revision")?,
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
            .map_err(error)?,
        event,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: payload,
        run: None,
        evidence: vec![],
        findings: vec![],
    })
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
