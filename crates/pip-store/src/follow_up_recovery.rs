//! An operator may correct only the proven ready/feedback misclassification.
use super::*;

pub(crate) fn validate(
    tx: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<()> {
    let invalid = || StoreError::InvalidInput("invalid false-takeover recovery");
    let payload = &input.event.payload;
    let request = &payload["request"];
    let reason = request["reason"].as_str().ok_or_else(invalid)?;
    if current.state != "TAKEN_OVER"
        || input.next_state != "PLANNING"
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
        || input.effects[0].effect_type != "DISPATCH_PLANNER"
    {
        return Err(invalid());
    }
    let proven: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM events t
         JOIN events f ON f.case_key=t.case_key AND f.state_revision=t.state_revision-1
         JOIN events r ON r.case_key=t.case_key AND r.state_revision=t.state_revision-2
         JOIN policies p ON p.repository_id=?3 AND p.revision=?4
         WHERE t.case_key=?1 AND t.state_revision=?2
         AND t.event_type='HUMAN_TOOK_OVER' AND t.previous_state='PLANNING' AND t.next_state='TAKEN_OVER'
         AND json_extract(t.payload_json,'$.blockers')='[\"PR_LEFT_DRAFT_STATE\"]'
         AND f.event_type='HUMAN_FEEDBACK_RECEIVED' AND f.previous_state='SHADOW_READY' AND f.next_state='PLANNING'
         AND json_extract(f.payload_json,'$.bounded')=0
         AND r.next_state='SHADOW_READY' AND r.event_type='READY'
         AND t.head_sha=f.head_sha AND f.head_sha=r.head_sha AND t.head_sha=?5
         AND t.pr_number=f.pr_number AND f.pr_number=r.pr_number AND t.pr_number=?6
         AND t.plan_version=f.plan_version AND f.plan_version=r.plan_version
         AND t.policy_revision=f.policy_revision AND f.policy_revision=r.policy_revision
         AND EXISTS(SELECT 1 FROM evidence e WHERE e.case_key=t.case_key
             AND e.kind='GITHUB_DISPOSITION_COMMENT' AND e.observed_at>=r.observed_at AND e.observed_at<=f.observed_at
             AND json_extract(e.payload_json,'$.effect_type')='NOTIFY_SHADOW_READY'
             AND json_extract(e.payload_json,'$.actor_id')=json_extract(p.payload_json,'$.github.automation_actor_id')
             AND json_extract(e.payload_json,'$.target_number')=t.pr_number
             AND json_extract(e.payload_json,'$.ready_for_review.pull_request_number')=t.pr_number
             AND json_extract(e.payload_json,'$.ready_for_review.head_sha')=t.head_sha
             AND json_extract(e.payload_json,'$.ready_for_review.mutation')='updated')
         AND NOT EXISTS(SELECT 1 FROM direct_attempts WHERE case_key=t.case_key AND status='RUNNING'))",
        params![current.case_key,sql_u64(current.state_revision)?,sql_u64(current.repository_id)?,sql_u64(current.policy_revision)?,current.head_sha,current.pr_number.map(sql_u64).transpose()?],
        |row|row.get(0),
    )?;
    if !proven {
        return Err(invalid());
    }
    Ok(())
}
