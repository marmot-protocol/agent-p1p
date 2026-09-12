//! Explicit operator recovery after an infrastructure stall exhausted review time.
use crate::builder_retry::{recovery_command, recovery_context};
use crate::{PublicationRetryRequest, RepositoryPolicy};
use pip_controller::LedgerController;
use pip_core::{Event, EventId, GitSha};
use pip_store::{ApplyResult, Store};
use serde_json::json;
use std::str::FromStr;

pub fn authorize_infrastructure_recovery(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &PublicationRetryRequest,
    now: u64,
    operator_uid: u32,
) -> Result<ApplyResult, String> {
    let (case, accepted) = recovery_context(store, paused, &request.case_key, operator_uid)?;
    let id = EventId::from_str(&request.request_id).map_err(error)?;
    GitSha::from_str(&request.expected_head).map_err(error)?;
    for event in store
        .immutable_history_for_case(&case.case_key)
        .map_err(error)?
        .events
    {
        if event.event_id == request.request_id {
            if event.event_type == "INFRASTRUCTURE_RECOVERY_AUTHORIZED"
                && event.payload["request"] == serde_json::to_value(request).map_err(error)?
                && event.payload["operator_uid"] == operator_uid
                && event.state_revision == request.expected_revision.saturating_add(1)
            {
                return Ok(ApplyResult::Replayed);
            }
            return Err("recovery request conflicts with recorded authorization".into());
        }
    }
    if case.state_revision != request.expected_revision
        || case.head_sha.as_deref() != Some(request.expected_head.as_str())
        || store.status(now).map_err(error)?.direct_attempts_running != 0
    {
        return Err("recovery requires exact revision/head and stopped direct work".into());
    }
    let limit = store
        .effective_provider_failure_limit(&case.case_key, u64::from(accepted.max_provider_failures))
        .map_err(error)?;
    if store
        .failed_direct_attempt_count_for_case(&case.case_key)
        .map_err(error)?
        >= limit
    {
        return Err("infrastructure recovery does not increase the provider failure budget".into());
    }
    let deadline = now
        .checked_add(accepted.max_case_elapsed_seconds)
        .ok_or("deadline overflow")?;
    let command = recovery_command(
        &case,
        request.expected_revision,
        id,
        Event::InfrastructureRecoveryAuthorized,
        json!({"schema_version":1,"operator_uid":operator_uid,"request":request,"deadline":deadline}),
        now,
    )?;
    LedgerController::apply(store, &accepted.case_policy(), &command).map_err(error)
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
