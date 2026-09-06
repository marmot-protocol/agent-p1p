//! Offline, root-authorized recovery of an exhausted pre-build task.
use std::num::{NonZeroU32, NonZeroU64};
use std::str::FromStr;

use pip_controller::{LedgerController, WorkflowCommand};
use pip_core::{
    CaseId, CaseState, Event, EventId, IssueNumber, ObservedAt, PlanVersion, PolicyRevision,
    RepositoryId, StateRevision, WorkflowVersion,
};
use pip_store::{ApplyResult, BuilderRetryAuthorization, Store};

use crate::{RepositoryPolicy, load_repository_policy};

#[derive(Clone, Debug)]
pub struct BuilderRetryRequest {
    pub case_key: String,
    pub expected_revision: u64,
    pub effect_id: String,
    pub expected_failures: u64,
    pub request_id: String,
    pub reason: String,
}

/// CLI checks actual process identity and stopped execution units before calling.
/// No policy, plan, terminal attempt, or old task payload is rewritten.
pub fn authorize_builder_retry(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &BuilderRetryRequest,
    now: u64,
    operator_uid: u32,
) -> Result<ApplyResult, String> {
    if operator_uid != 0 {
        return Err("builder retry requires root authorization".into());
    }
    if paused.intake.enabled || !paused.intake.paused || paused.dispatch_enabled {
        return Err("builder retry requires an inert installed policy".into());
    }
    let case = store
        .case(&request.case_key)
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
    let event_id = EventId::from_str(&request.request_id).map_err(error)?;
    let authorization = BuilderRetryAuthorization {
        schema_version: 1,
        effect_id: request.effect_id.clone(),
        failed_attempts: request.expected_failures,
        base_failure_limit: accepted.max_provider_failures,
        operator_uid,
        reason: request.reason.clone(),
    };
    let payload = serde_json::to_value(authorization).map_err(error)?;
    // Retry the same operator request safely, even after the new job advances.
    if let Some(event) = store
        .immutable_history_for_case(&case.case_key)
        .map_err(error)?
        .events
        .iter()
        .find(|event| event.event_id == request.request_id)
    {
        if event.event_type == "BUILDER_RETRY_AUTHORIZED"
            && event.payload == payload
            && request.expected_revision.checked_add(1) == Some(event.state_revision)
        {
            return Ok(ApplyResult::Replayed);
        }
        return Err("retry request id conflicts with recorded authorization".into());
    }
    let command = WorkflowCommand {
        case_id: CaseId::new(
            RepositoryId::new(NonZeroU64::new(case.repository_id).ok_or("invalid repository")?),
            IssueNumber::new(NonZeroU64::new(case.issue_number).ok_or("invalid issue")?),
            WorkflowVersion::new(NonZeroU32::new(case.workflow_version).ok_or("invalid workflow")?),
        ),
        event_id,
        observed_at: ObservedAt::new(now),
        expected_state: CaseState::ReadyToBuild,
        expected_state_revision: StateRevision::new(
            NonZeroU64::new(request.expected_revision).ok_or("invalid state revision")?,
        ),
        accepted_policy_revision: PolicyRevision::new(
            NonZeroU64::new(case.policy_revision).ok_or("invalid policy revision")?,
        ),
        remediation_round: case.remediation_round,
        plan_version: NonZeroU32::new(case.plan_version).map(PlanVersion::new),
        pr_number: None,
        head_sha: None,
        event: Event::BuilderRetryAuthorized,
        accepted_plan_version: None,
        next_pr_number: None,
        next_head_sha: None,
        event_payload: payload,
        run: None,
        evidence: vec![],
        findings: vec![],
    };
    LedgerController::apply(store, &accepted.case_policy(), &command).map_err(error)
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
