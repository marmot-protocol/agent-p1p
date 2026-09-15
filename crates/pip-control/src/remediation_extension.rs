//! Explicit, offline increase of a CI-escalated case's remediation ceiling.
use crate::builder_retry::{recovery_command, recovery_context};
use crate::{PublicationRetryRequest, RepositoryPolicy};
use pip_controller::LedgerController;
use pip_core::{Event, EventId, GitSha};
use pip_store::{ApplyResult, Store};
use serde_json::json;
use std::str::FromStr;

pub fn authorize_remediation_extension(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &PublicationRetryRequest,
    now: u64,
    operator_uid: u32,
) -> Result<ApplyResult, String> {
    let (case, accepted) = recovery_context(store, paused, &request.case_key, operator_uid)?;
    let id = EventId::from_str(&request.request_id).map_err(error)?;
    GitSha::from_str(&request.expected_head).map_err(error)?;
    let mut next = paused.clone();
    next.intake.enabled = accepted.intake.enabled;
    next.intake.paused = accepted.intake.paused;
    next.dispatch_enabled = accepted.dispatch_enabled;
    // Match intake's immutable case-policy representation. Conversation intake
    // is an independent host switch, not a change to case authority.
    next.conversations_enabled = false;
    let policy = serde_json::to_value(&next).map_err(error)?;
    for event in store
        .immutable_history_for_case(&case.case_key)
        .map_err(error)?
        .events
    {
        if event.event_id == request.request_id {
            if event.event_type == "REMEDIATION_BUDGET_EXTENDED"
                && event.payload["request"] == serde_json::to_value(request).map_err(error)?
                && event.payload["policy"] == policy
                && event.payload["operator_uid"] == operator_uid
                && event.state_revision == request.expected_revision.saturating_add(1)
            {
                return Ok(ApplyResult::Replayed);
            }
            return Err("budget extension conflicts with recorded request".into());
        }
    }
    if case.state_revision != request.expected_revision
        || case.head_sha.as_deref() != Some(request.expected_head.as_str())
        || store.status(now).map_err(error)?.direct_attempts_running != 0
    {
        return Err("budget extension requires exact revision/head and stopped work".into());
    }
    let command = recovery_command(
        &case,
        request.expected_revision,
        id,
        Event::RemediationBudgetExtended,
        json!({"schema_version":1,"operator_uid":operator_uid,"request":request,"policy":policy}),
        now,
    )?;
    LedgerController::apply(store, &accepted.case_policy(), &command).map_err(error)
}
fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
