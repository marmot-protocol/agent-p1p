//! Offline authorization to repair a legacy publication of accepted work.
use crate::{
    RepositoryPolicy,
    builder_retry::{recovery_command, recovery_context},
    draft_pr::{payload_matches, planned_base, publication_source},
};
use pip_controller::LedgerController;
use pip_core::{Event, EventId, GitSha};
use pip_store::{ApplyResult, ImmutableCaseHistory, Store, StoredCase};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationRetryRequest {
    pub case_key: String,
    pub expected_revision: u64,
    pub expected_head: String,
    pub request_id: String,
    pub reason: String,
}

pub fn authorize_publication_retry(
    store: &mut Store,
    paused: &RepositoryPolicy,
    request: &PublicationRetryRequest,
    now: u64,
    operator_uid: u32,
) -> Result<ApplyResult, String> {
    let (case, accepted) = recovery_context(store, paused, &request.case_key, operator_uid)?;
    let event_id = EventId::from_str(&request.request_id).map_err(error)?;
    GitSha::from_str(&request.expected_head).map_err(error)?;
    if request.reason.trim().is_empty()
        || request.reason.len() > 2000
        || request.reason.chars().any(char::is_control)
    {
        return Err("publication retry requires a bounded human reason".into());
    }
    let history = store
        .immutable_history_for_case(&case.case_key)
        .map_err(error)?;
    if let Some(event) = history
        .events
        .iter()
        .find(|event| event.event_id == request.request_id)
    {
        let authorization =
            serde_json::from_value::<Authorization>(event.payload.clone()).map_err(error)?;
        if event.event_type == "PUBLICATION_RETRY_AUTHORIZED"
            && authorization.schema_version == 1
            && authorization.operator_uid == operator_uid
            && authorization.request == *request
            && request.expected_revision.checked_add(1) == Some(event.state_revision)
            && payload_matches(&event.payload, &event.payload_sha256)
        {
            return Ok(ApplyResult::Replayed);
        }
        return Err("publication retry request conflicts with recorded authorization".into());
    }
    if case.state_revision != request.expected_revision
        || case.head_sha.as_deref() != Some(request.expected_head.as_str())
    {
        return Err("publication retry requires the exact current revision and head".into());
    }
    if store.status(now).map_err(error)?.direct_attempts_running != 0 {
        return Err("publication retry requires stopped direct attempts".into());
    }
    let (_, build_event_id) = recoverable_source(&history, &case)?;
    let authorization = Authorization {
        schema_version: 1,
        operator_uid,
        request: request.clone(),
        build_event_id,
        parent_head: planned_base(&history, &case).map_err(error)?.to_string(),
    };
    let command = recovery_command(
        &case,
        request.expected_revision,
        event_id,
        Event::PublicationRetryAuthorized,
        serde_json::to_value(authorization).map_err(error)?,
        now,
    )?;
    LedgerController::apply(store, &accepted.case_policy(), &command).map_err(error)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    schema_version: u32,
    operator_uid: u32,
    request: PublicationRetryRequest,
    build_event_id: String,
    parent_head: String,
}

pub(crate) fn authorized_build(
    history: &ImmutableCaseHistory,
    case: &StoredCase,
) -> Result<(pip_contracts::BuilderResult, GitSha), String> {
    let event = history
        .events
        .last()
        .ok_or("missing recovery authorization")?;
    let auth: Authorization = serde_json::from_value(event.payload.clone()).map_err(error)?;
    if event.event_type != "PUBLICATION_RETRY_AUTHORIZED"
        || event.state_revision != case.state_revision
        || !payload_matches(&event.payload, &event.payload_sha256)
        || auth.schema_version != 1
        || auth.operator_uid != 0
        || auth.request.case_key != case.case_key
        || auth.request.request_id != event.event_id
        || auth.request.expected_revision.checked_add(1) != Some(case.state_revision)
        || case.head_sha.as_deref() != Some(auth.request.expected_head.as_str())
    {
        return Err("invalid publication recovery binding".into());
    }
    let (build, event_id) = recoverable_source(history, case)?;
    let parent = planned_base(history, case).map_err(error)?;
    if event_id != auth.build_event_id || parent.to_string() != auth.parent_head {
        return Err("publication recovery source changed".into());
    }
    Ok((build, parent))
}

fn recoverable_source(
    history: &ImmutableCaseHistory,
    case: &StoredCase,
) -> Result<(pip_contracts::BuilderResult, String), String> {
    let publication = &history
        .events
        .iter()
        .rev()
        .find(|event| event.event_type == "REVIEW_READY")
        .ok_or("missing publication")?
        .payload["publication"];
    let signing = &publication["signing"];
    // Old unsigned publications and the original single-parent signer may be
    // repaired. Do not repeatedly republish a target-aware signed result.
    if !signing["integrated_base"].is_null() {
        return Err("publication already binds integrated target ancestry".into());
    }
    publication_source(history, case, !signing.is_null()).map_err(error)
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}
