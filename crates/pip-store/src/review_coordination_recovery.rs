//! Audited recovery of a lost required peer or an operator-cleared feedback hold.
use super::*;
use serde_json::json;

pub(crate) fn validate(
    tx: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<()> {
    let invalid = || StoreError::InvalidInput("invalid review coordination recovery");
    let payload = &input.event.payload;
    let request = &payload["request"];
    let reason = request["reason"].as_str().ok_or_else(invalid)?;
    if !matches!(current.state.as_str(), "REVIEWING" | "ESCALATED")
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
    let idle: bool = tx.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM direct_attempts WHERE case_key=?1 AND status='RUNNING')
         AND NOT EXISTS(SELECT 1 FROM outbox WHERE case_key=?1 AND (lease_owner IS NOT NULL OR lease_until IS NOT NULL OR (delivered_at IS NULL AND superseded_at IS NULL AND effect_type!='ESCALATE')))
         AND NOT EXISTS(SELECT 1 FROM events WHERE case_key=?1 AND event_type='REVIEW_COORDINATION_RECOVERY_AUTHORIZED')
         AND EXISTS(SELECT 1 FROM runs WHERE case_key=?1 AND role='builder')",[&current.case_key],|row|row.get(0))?;
    if !idle {
        return Err(invalid());
    }
    let recoverable = if current.state == "ESCALATED" {
        let raw: Option<String> = tx.query_row(
            "SELECT payload_json FROM events WHERE case_key=?1 AND state_revision=?2
             AND event_type='OPERATIONAL_BOUND_REACHED' AND previous_state='FINAL_REVIEW' AND next_state='ESCALATED'",
            params![current.case_key,sql_u64(current.state_revision)?],|row|row.get(0)).optional()?;
        let stop: Value = raw
            .map(|raw| serde_json::from_str(&raw))
            .transpose()?
            .unwrap_or(Value::Null);
        stop["reason"] == "REVIEW_FEEDBACK_ALREADY_ATTEMPTED"
            && stop["head_sha"].as_str() == current.head_sha.as_deref()
            && stop["pull_request_number"].as_u64() == current.pr_number
            && stop["blockers"].as_array().is_some_and(|blockers| {
                !blockers.is_empty()
                    && blockers.iter().all(|b| {
                        b.as_str()
                            .is_some_and(|b| b.starts_with("UNRESOLVED_REVIEW_THREAD:"))
                    })
            })
    } else {
        let mut statement = tx.prepare(
            "SELECT o.effect_id,o.payload_json FROM outbox o
             JOIN events stop ON stop.event_id=o.superseded_by_event_id AND stop.case_key=o.case_key
             JOIN events origin ON origin.case_key=o.case_key AND origin.state_revision=o.state_revision
             WHERE o.case_key=?1 AND o.effect_type='RUN_DIRECT_WORKER'
               AND o.delivered_at IS NULL AND o.superseded_at IS NOT NULL
               AND origin.next_state='REVIEWING' AND stop.event_type='REVIEW_RECORDED'
               AND NOT EXISTS(SELECT 1 FROM direct_attempts a WHERE a.effect_id=o.effect_id)
               AND NOT EXISTS(SELECT 1 FROM events e WHERE e.case_key=o.case_key AND e.state_revision>o.state_revision AND e.event_type!='REVIEW_RECORDED')")?;
        let rows = statement.query_map([&current.case_key], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut found = false;
        for row in rows {
            let (effect, raw) = row?;
            let desired = dispatch_intents::resolve_output(
                tx,
                &effect,
                DispatchTransport::Direct,
                serde_json::from_str(&raw)?,
            )?;
            let body = &desired["body"];
            found |= matches!(
                body["role"].as_str(),
                Some("reviewer-general" | "reviewer-secperf")
            ) && body["review_mode"] == "required"
                && body["expected_head_sha"].as_str() == current.head_sha.as_deref()
                && body["plan_version"].as_u64() == Some(u64::from(current.plan_version))
                && body["remediation_round"].as_u64() == Some(u64::from(current.remediation_round))
                && body["pr_number"].as_u64() == current.pr_number;
        }
        found
    };
    let expected = json!({"case_key":current.case_key,"state_revision":current.state_revision+1,"effect":"OBSERVE_CI","remediation_round":current.remediation_round,"plan_version":current.plan_version,"pr_number":current.pr_number,"head_sha":current.head_sha});
    if !recoverable || input.effects[0].payload != expected {
        return Err(invalid());
    }
    Ok(())
}
