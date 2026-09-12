//! Deadline recovery is an explicit, bounded operator event, never a clock reset.
use super::*;

pub(crate) fn validate(
    transaction: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<()> {
    let invalid = || StoreError::InvalidInput("invalid infrastructure recovery authorization");
    let payload = &input.event.payload;
    let request = &payload["request"];
    let reason = request["reason"].as_str().ok_or_else(invalid)?;
    if current.state != "ESCALATED"
        || input.next_state != "REMEDIATING"
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
        || current.remediation_round.checked_add(1) != Some(input.remediation_round)
        || reason.trim().is_empty()
        || reason.len() > 2000
        || reason.chars().any(char::is_control)
        || input.run.is_some()
        || !input.evidence.is_empty()
        || !input.findings.is_empty()
        || input.effects.len() != 1
        || input.effects[0].effect_type != "DISPATCH_BUILDER"
    {
        return Err(invalid());
    }
    let recoverable: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE case_key=?1 AND state_revision=?2
         AND event_type='OPERATIONAL_BOUND_REACHED' AND next_state='ESCALATED'
         AND previous_state='REVIEWING' AND observed_at<=?3
         AND json_extract(payload_json,'$.bound')='ELAPSED_TIME')",
        params![
            current.case_key,
            sql_u64(current.state_revision)?,
            sql_u64(input.observed_at)?
        ],
        |row| row.get(0),
    )?;
    let accepted: String = transaction.query_row(
        "SELECT payload_json FROM policies WHERE repository_id=?1 AND revision=?2",
        params![
            sql_u64(current.repository_id)?,
            sql_u64(current.policy_revision)?
        ],
        |row| row.get(0),
    )?;
    let policy: Value = serde_json::from_str(&accepted)?;
    let duration = policy["max_case_elapsed_seconds"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(invalid)?;
    let deadline = input
        .observed_at
        .checked_add(duration)
        .ok_or_else(invalid)?;
    let running: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM direct_attempts WHERE case_key=?1 AND status='RUNNING'",
        [&current.case_key],
        |row| row.get(0),
    )?;
    let builds: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM runs WHERE case_key=?1 AND role='builder'",
        [&current.case_key],
        |row| row.get(0),
    )?;
    if !recoverable
        || running != 0
        || builds == 0
        || payload["deadline"].as_u64() != Some(deadline)
        || u64::from(input.remediation_round)
            > policy["max_remediation_rounds"]
                .as_u64()
                .ok_or_else(invalid)?
    {
        return Err(invalid());
    }
    Ok(())
}

impl Store {
    pub fn infrastructure_recovery_deadline(&self, case_key: &str) -> Result<Option<u64>> {
        let deadline: Option<i64> = self.connection.query_row(
            "SELECT MAX(json_extract(payload_json,'$.deadline')) FROM events
             WHERE case_key=?1 AND event_type='INFRASTRUCTURE_RECOVERY_AUTHORIZED'",
            [case_key],
            |row| row.get(0),
        )?;
        Ok(deadline.map(unsigned))
    }
}
