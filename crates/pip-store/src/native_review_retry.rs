//! Bounded operator recovery of a native required-review startup failure.
use super::*;
use serde_json::json;

pub(crate) fn validate(
    tx: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<()> {
    let invalid = || StoreError::InvalidInput("invalid native review retry");
    let payload = &input.event.payload;
    let request = &payload["request"];
    let reason = request["reason"].as_str().ok_or_else(invalid)?;
    if current.state != "ESCALATED"
        || input.next_state != "WAITING_CI"
        || payload["schema_version"] != 1
        || payload["operator_uid"] != 0
        || request["case_key"] != current.case_key
        || request["expected_revision"].as_u64() != Some(current.state_revision)
        || request["request_id"] != input.event.event_id
        || request["expected_head"].as_str() != current.head_sha.as_deref()
        || current.pr_number.is_none()
        || current.head_sha.is_none()
        || current.plan_version == 0
        || input.plan_version != current.plan_version
        || input.pr_number != current.pr_number
        || input.head_sha != current.head_sha
        || input.remediation_round != current.remediation_round
        || reason.trim().is_empty()
        || reason.len() > 2000
        || reason.chars().any(char::is_control)
        || input.run.is_some()
        || !input.findings.is_empty()
        || !input.evidence.is_empty()
        || input.effects.len() != 1
        || input.effects[0].effect_type != "OBSERVE_CI"
    {
        return Err(invalid());
    }
    // Bind to the failed projection and its immutable dispatch, not an operator
    // supplied task/model. A completed result or a different hold stays held.
    let row: Option<(String,String,String)> = tx.query_row(
        "SELECT stop.payload_json,p.effect_id,p.desired_json FROM events stop
         JOIN task_projections p ON p.case_key=stop.case_key AND p.task_id=json_extract(stop.payload_json,'$.details.task_id')
         JOIN outbox o ON o.effect_id=p.effect_id AND o.case_key=stop.case_key
         WHERE stop.case_key=?1 AND stop.state_revision=?2
           AND stop.event_type='OPERATIONAL_BOUND_REACHED' AND stop.previous_state='REVIEWING' AND stop.next_state='ESCALATED'
           AND json_extract(stop.payload_json,'$.bound')='PROVIDER_FAILURES'
           AND json_extract(stop.payload_json,'$.details.source')='hermes-circuit-breaker'
           AND o.effect_type='DISPATCH_REVIEWERS' AND o.delivered_at IS NOT NULL
           AND NOT EXISTS(SELECT 1 FROM evidence WHERE case_key=?1 AND kind='HERMES_RESULT' AND source=p.projection_id)
           AND NOT EXISTS(SELECT 1 FROM runs WHERE case_key=?1 AND task_id=p.task_id)
           AND NOT EXISTS(SELECT 1 FROM direct_attempts WHERE case_key=?1 AND status='RUNNING')
           AND NOT EXISTS(SELECT 1 FROM outbox WHERE case_key=?1 AND (lease_owner IS NOT NULL OR lease_until IS NOT NULL OR (delivered_at IS NULL AND superseded_at IS NULL AND effect_type!='ESCALATE')))
           AND NOT EXISTS(SELECT 1 FROM events WHERE case_key=?1 AND event_type='NATIVE_REVIEW_RETRY_AUTHORIZED')
           AND EXISTS(SELECT 1 FROM runs WHERE case_key=?1 AND role='planner')
           AND EXISTS(SELECT 1 FROM runs WHERE case_key=?1 AND role='builder')",
        params![current.case_key,sql_u64(current.state_revision)?],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?;
    let (stop, effect, raw) = row.ok_or_else(invalid)?;
    let stop: Value = serde_json::from_str(&stop)?;
    let desired = dispatch_intents::resolve_output(
        tx,
        &effect,
        dispatch_intents::DispatchTransport::Hermes,
        serde_json::from_str(&raw)?,
    )?;
    let body = &desired["body"];
    if body["role"] != "reviewer-general"
        || body["review_mode"] != "required"
        || body["expected_head_sha"].as_str() != current.head_sha.as_deref()
        || body["plan_version"].as_u64() != Some(u64::from(current.plan_version))
        || body["state_revision"].as_u64() != current.state_revision.checked_sub(1)
        || stop["details"]["profile"] != desired["assignee"]
        || stop["details"]["provider"] != desired["provider"]
        || stop["details"]["model"] != desired["model"]
        || stop["limit"] != desired["max_retries"]
        || stop["observed"] != stop["limit"]
    {
        return Err(invalid());
    }
    let expected = json!({"case_key":current.case_key,"state_revision":current.state_revision+1,"effect":"OBSERVE_CI","remediation_round":current.remediation_round,"plan_version":current.plan_version,"pr_number":current.pr_number,"head_sha":current.head_sha});
    if input.effects[0].payload != expected {
        return Err(invalid());
    }
    Ok(())
}
