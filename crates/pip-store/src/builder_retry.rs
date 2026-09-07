//! An operator-authorized extra attempt, recorded in the existing immutable event log.
use super::*;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuilderRetryAuthorization {
    pub schema_version: u32,
    pub effect_id: String,
    pub failed_attempts: u64,
    pub base_failure_limit: u32,
    pub operator_uid: u32,
    pub reason: String,
}

pub(crate) fn validate_retry(
    transaction: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<()> {
    let review = input.event.event_type == "REVIEW_RETRY_AUTHORIZED";
    let next_state = if review {
        "WAITING_CI"
    } else {
        "READY_TO_BUILD"
    };
    let effect_type = if review {
        "OBSERVE_CI"
    } else {
        "DISPATCH_BUILDER"
    };
    let invalid = || {
        StoreError::InvalidInput(
            "work retry requires an exhausted, unleased task, unchanged accepted work, and root authorization",
        )
    };
    let authorization: BuilderRetryAuthorization =
        serde_json::from_value(input.event.payload.clone())?;
    if authorization.schema_version != 1
        || authorization.operator_uid != 0
        || authorization.reason.trim().is_empty()
        || authorization.reason.len() > 2000
        || authorization.reason.chars().any(char::is_control)
        || !(current.state == "ESCALATED" || !review && current.state == "READY_TO_BUILD")
        || input.next_state != next_state
        || current.plan_version == 0
        || input.plan_version != current.plan_version
        || input.remediation_round != current.remediation_round
        || current.pr_number.is_some() != review
        || current.head_sha.is_some() != review
        || input.pr_number != current.pr_number
        || input.head_sha != current.head_sha
        || input.run.is_some()
        || !input.evidence.is_empty()
        || !input.findings.is_empty()
        || input.effects.len() != 1
        || input.effects[0].effect_type != effect_type
    {
        return Err(invalid());
    }
    let accepted: String = transaction.query_row(
        "SELECT payload_json FROM policies WHERE repository_id=?1 AND revision=?2",
        params![
            sql_u64(current.repository_id)?,
            sql_u64(current.policy_revision)?
        ],
        |row| row.get(0),
    )?;
    let accepted: Value = serde_json::from_str(&accepted)?;
    let limit = accepted["max_provider_failures"]
        .as_u64()
        .ok_or_else(invalid)?;
    let elapsed_limit = accepted["max_case_elapsed_seconds"]
        .as_u64()
        .ok_or_else(invalid)?;
    let (created, updated): (i64, i64) = transaction.query_row(
        "SELECT created_at, updated_at FROM cases WHERE case_key=?1",
        [&current.case_key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if limit == 0
        || u64::from(authorization.base_failure_limit) != limit
        || input.observed_at < unsigned(updated)
        || input.observed_at.saturating_sub(unsigned(created)) >= elapsed_limit
        || authorization.failed_attempts < limit
    {
        return Err(invalid());
    }
    let failed: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM direct_attempts WHERE case_key=?1 AND status='FAILED'",
        [&current.case_key],
        |row| row.get(0),
    )?;
    if unsigned(failed) != authorization.failed_attempts {
        return Err(invalid());
    }
    // Only an exact direct-worker failure-bound escalation is recoverable here.
    // Scope, elapsed-time, review and other terminal decisions stay held.
    let escalated = current.state == "ESCALATED";
    let effect_revision = if escalated {
        let recoverable: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE case_key=?1 AND state_revision=?2
             AND event_type='OPERATIONAL_BOUND_REACHED' AND previous_state=?3
             AND next_state='ESCALATED' AND json_extract(payload_json,'$.bound')='PROVIDER_FAILURES'
             AND json_extract(payload_json,'$.details.source')='direct-worker')",
            params![
                current.case_key,
                sql_u64(current.state_revision)?,
                if review {
                    "REVIEWING"
                } else {
                    "READY_TO_BUILD"
                }
            ],
            |row| row.get(0),
        )?;
        if !recoverable {
            return Err(invalid());
        }
        current.state_revision.checked_sub(1).ok_or_else(invalid)?
    } else {
        current.state_revision
    };
    // Check under the same IMMEDIATE transaction as supersession and dispatch.
    let admissible: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM outbox WHERE effect_id=?1 AND case_key=?2 AND state_revision=?3
          AND effect_type='RUN_DIRECT_WORKER' AND json_extract(payload_json,'$.role')=?7
          AND (?8=0 OR json_extract(payload_json,'$.body.review_mode')='required')
          AND delivered_at IS NULL AND (superseded_at IS NOT NULL)=?5 AND lease_owner IS NULL AND lease_until IS NULL)
         AND NOT EXISTS(SELECT 1 FROM direct_attempts WHERE case_key=?2 AND (status='RUNNING' OR (status='COMPLETE' AND (?8=0 OR effect_id=?1))))
         AND (SELECT COUNT(*) FROM outbox WHERE case_key=?2 AND delivered_at IS NULL AND superseded_at IS NULL AND effect_type!='ESCALATE')=?6
         AND NOT EXISTS(SELECT 1 FROM outbox WHERE case_key=?2 AND (lease_owner IS NOT NULL OR lease_until IS NOT NULL))
         AND EXISTS(SELECT 1 FROM direct_attempts WHERE effect_id=?1 AND case_key=?2 AND status='FAILED')
         AND EXISTS(SELECT 1 FROM runs WHERE case_key=?2 AND role='planner')
         AND (?8=0 OR EXISTS(SELECT 1 FROM runs WHERE case_key=?2 AND role='builder'))
         AND NOT EXISTS(SELECT 1 FROM events WHERE case_key=?2 AND event_type IN ('BUILDER_RETRY_AUTHORIZED','REVIEW_RETRY_AUTHORIZED')
             AND json_extract(payload_json,'$.failed_attempts')>=?4)",
        params![authorization.effect_id,current.case_key,sql_u64(effect_revision)?,failed,escalated,if escalated {0} else {1},if review { "reviewer-secperf" } else { "builder" },review],|row|row.get(0))?;
    if !admissible {
        return Err(invalid());
    }
    authorization
        .failed_attempts
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(invalid)?;
    let expected = serde_json::json!({"case_key":current.case_key,"state_revision":current.state_revision+1,
        "effect":effect_type,"remediation_round":current.remediation_round,"plan_version":current.plan_version,
        "pr_number":current.pr_number,"head_sha":current.head_sha});
    if input.effects[0].payload != expected {
        return Err(invalid());
    }
    Ok(())
}

impl Store {
    /// A retry contributes exactly one failure allowance, never erases failures.
    /// Successful later stages can proceed, but another failure reaches the new bound.
    pub fn effective_provider_failure_limit(&self, case_key: &str, base: u64) -> Result<u64> {
        let granted: Option<i64> = self.connection.query_row(
            "SELECT MAX(json_extract(payload_json,'$.failed_attempts')+1) FROM events
             WHERE case_key=?1 AND ((event_type='BUILDER_RETRY_AUTHORIZED'
               AND previous_state IN ('READY_TO_BUILD','ESCALATED') AND next_state='READY_TO_BUILD')
               OR (event_type='REVIEW_RETRY_AUTHORIZED' AND previous_state='ESCALATED' AND next_state='WAITING_CI'))",
            [case_key],
            |row| row.get(0),
        )?;
        Ok(base.max(granted.map(unsigned).unwrap_or(0)))
    }

    pub fn accepted_policy(&self, repository_id: u64, revision: u64) -> Result<Value> {
        let value: String = self.connection.query_row(
            "SELECT payload_json FROM policies WHERE repository_id=?1 AND revision=?2",
            params![sql_u64(repository_id)?, sql_u64(revision)?],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&value)?)
    }
}
