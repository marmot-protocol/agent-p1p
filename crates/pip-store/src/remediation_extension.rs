//! Atomic policy-budget extension without rewriting previous case authority.
use super::*;

pub(crate) fn validate(
    tx: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<u64> {
    let invalid = || StoreError::InvalidInput("invalid remediation budget extension");
    let event = &input.event.payload;
    let request = &event["request"];
    let new = &event["policy"];
    let reason = request["reason"].as_str().ok_or_else(invalid)?;
    let revision = new["revision"].as_u64().ok_or_else(invalid)?;
    let new_limit = new["max_remediation_rounds"].as_u64().ok_or_else(invalid)?;
    if current.state != "ESCALATED"
        || input.next_state != "WAITING_CI"
        || event["schema_version"] != 1
        || event["operator_uid"] != 0
        || request["case_key"] != current.case_key
        || request["expected_revision"].as_u64() != Some(current.state_revision)
        || request["expected_head"].as_str() != current.head_sha.as_deref()
        || request["request_id"] != input.event.event_id
        || reason.trim().is_empty()
        || reason.len() > 2000
        || reason.chars().any(char::is_control)
        || revision <= current.policy_revision
        || new_limit > u64::from(u32::MAX)
        || current.pr_number.is_none()
        || current.head_sha.is_none()
        || current.plan_version == 0
        || input.pr_number != current.pr_number
        || input.head_sha != current.head_sha
        || input.plan_version != current.plan_version
        || input.remediation_round != current.remediation_round
        || input.run.is_some()
        || !input.evidence.is_empty()
        || !input.findings.is_empty()
        || input.effects.len() != 1
        || input.effects[0].effect_type != "OBSERVE_CI"
    {
        return Err(invalid());
    }
    let old: String = tx.query_row(
        "SELECT payload_json FROM policies WHERE repository_id=?1 AND revision=?2",
        params![
            sql_u64(current.repository_id)?,
            sql_u64(current.policy_revision)?
        ],
        |row| row.get(0),
    )?;
    let old: Value = serde_json::from_str(&old)?;
    let old_limit = old["max_remediation_rounds"].as_u64().ok_or_else(invalid)?;
    let mut expected = old.clone();
    expected["revision"] = Value::from(revision);
    expected["max_remediation_rounds"] = Value::from(new_limit);
    if &expected != new
        || new_limit <= old_limit
        || u64::from(current.remediation_round) < old_limit
        || u64::from(current.remediation_round) >= new_limit
    {
        return Err(invalid());
    }
    let eligible: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM events WHERE case_key=?1 AND state_revision=?2 AND event_type='CI_FAILED' AND previous_state='WAITING_CI' AND next_state='ESCALATED' AND observed_at<=?3)
        AND NOT EXISTS(SELECT 1 FROM direct_attempts WHERE case_key=?1 AND status='RUNNING')
        AND NOT EXISTS(SELECT 1 FROM outbox WHERE case_key=?1 AND lease_owner IS NOT NULL)",params![current.case_key,sql_u64(current.state_revision)?,sql_u64(input.observed_at)?],|row|row.get(0))?;
    let authorized: i64 = tx.query_row("SELECT COALESCE((SELECT MAX(observed_at) FROM events WHERE case_key=?1 AND event_type='ISSUE_REAUTHORIZED'),created_at) FROM cases WHERE case_key=?1",[&current.case_key],|row|row.get(0))?;
    let recovery: Option<i64> = tx.query_row("SELECT MAX(json_extract(payload_json,'$.deadline')) FROM events WHERE case_key=?1 AND event_type='INFRASTRUCTURE_RECOVERY_AUTHORIZED'",[&current.case_key],|row|row.get(0))?;
    let deadline = unsigned(authorized)
        .saturating_add(
            old["max_case_elapsed_seconds"]
                .as_u64()
                .ok_or_else(invalid)?,
        )
        .max(recovery.map(unsigned).unwrap_or(0));
    if !eligible || input.observed_at >= deadline {
        return Err(invalid());
    }
    let (body, hash) = payload(new)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT payload_sha256 FROM policies WHERE repository_id=?1 AND revision=?2",
            params![sql_u64(current.repository_id)?, sql_u64(revision)?],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != hash {
            return Err(invalid());
        }
    } else {
        tx.execute("INSERT INTO policies(repository_id,revision,payload_json,payload_sha256,accepted_at) VALUES (?1,?2,?3,?4,?5)",params![sql_u64(current.repository_id)?,sql_u64(revision)?,body,hash,sql_u64(input.observed_at)?])?;
    }
    Ok(revision)
}
