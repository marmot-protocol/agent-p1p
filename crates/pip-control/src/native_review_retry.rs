//! One audited native reviewer retry after a diagnosed Hermes startup failure.
use crate::builder_retry::{recovery_command, recovery_context};
use crate::{PublicationRetryRequest, RepositoryPolicy};
use pip_controller::LedgerController;
use pip_core::{Event, EventId, GitSha};
use pip_store::{ApplyResult, Store};
use serde_json::json;
use std::str::FromStr;

pub fn authorize_native_review_retry(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &PublicationRetryRequest,
    now: u64,
    operator_uid: u32,
) -> Result<ApplyResult, String> {
    authorize(
        store,
        paused,
        request,
        now,
        operator_uid,
        Event::NativeReviewRetryAuthorized,
    )
}

pub fn authorize_review_coordination_recovery(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &PublicationRetryRequest,
    now: u64,
    operator_uid: u32,
) -> Result<ApplyResult, String> {
    authorize(
        store,
        paused,
        request,
        now,
        operator_uid,
        Event::ReviewCoordinationRecoveryAuthorized,
    )
}

fn authorize(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &PublicationRetryRequest,
    now: u64,
    operator_uid: u32,
    recovery_event: Event,
) -> Result<ApplyResult, String> {
    let error = |e: pip_store::StoreError| e.to_string();
    let (case, accepted) = recovery_context(store, paused, &request.case_key, operator_uid)?;
    let id = EventId::from_str(&request.request_id).map_err(|e| e.to_string())?;
    GitSha::from_str(&request.expected_head).map_err(|e| e.to_string())?;
    let payload = json!({"schema_version":1,"operator_uid":operator_uid,"request":request});
    let history = store
        .immutable_history_for_case(&case.case_key)
        .map_err(error)?;
    if let Some(event) = history
        .events
        .iter()
        .find(|event| event.event_id == request.request_id)
    {
        if event.event_type == recovery_event.to_string()
            && event.payload == payload
            && Some(event.state_revision) == request.expected_revision.checked_add(1)
        {
            return Ok(ApplyResult::Replayed);
        }
        return Err("recovery request conflicts with recorded authorization".into());
    }
    let created = store
        .case_authorized_at(&case.case_key)
        .map_err(error)?
        .ok_or("missing authorization")?;
    let deadline = created
        .saturating_add(accepted.max_case_elapsed_seconds)
        .max(
            store
                .infrastructure_recovery_deadline(&case.case_key)
                .map_err(error)?
                .unwrap_or(0),
        );
    let failure_limit = store
        .effective_provider_failure_limit(&case.case_key, u64::from(accepted.max_provider_failures))
        .map_err(error)?;
    if case.state_revision != request.expected_revision
        || case.head_sha.as_deref() != Some(&request.expected_head)
        || now >= deadline
        || store
            .running_direct_attempts_for_case(&case.case_key)
            .map_err(error)?
            != 0
        || store
            .failed_direct_attempt_count_for_case(&case.case_key)
            .map_err(error)?
            >= failure_limit
    {
        return Err("review recovery requires exact revision/head, stopped work and remaining time/failure budget".into());
    }
    let command = recovery_command(
        &case,
        request.expected_revision,
        id,
        recovery_event,
        payload,
        now,
    )?;
    LedgerController::apply(store, &accepted.case_policy(), &command).map_err(|e| e.to_string())
}
