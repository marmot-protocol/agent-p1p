//! Reauthorization appends a new planning generation, never resets case history.
use super::*;

fn eligible(
    connection: &Connection,
    case_key: &str,
    label_id: u64,
    removal_id: u64,
) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM cases c JOIN events e
          ON e.case_key=c.case_key AND e.state_revision=c.state_revision
          WHERE c.case_key=?1 AND c.state='ABANDONED' AND c.pr_number IS NULL AND c.head_sha IS NULL
          AND e.event_type='AUTHORIZATION_REMOVED'
          AND EXISTS(SELECT 1 FROM json_each(e.payload_json,'$.blockers')
              WHERE value IN ('REQUIRED_LABEL_MISSING','LATEST_AUTHORIZATION_REMOVED'))
          AND ?2 > ?3 AND ?3 > COALESCE((SELECT MAX(json_extract(payload_json,'$.label_event_id'))
              FROM events WHERE case_key=?1 AND event_type IN ('ISSUE_AUTHORIZED','ISSUE_REAUTHORIZED')),9223372036854775807)
          AND NOT EXISTS(SELECT 1 FROM direct_attempts WHERE case_key=?1 AND status='RUNNING')
          AND NOT EXISTS(SELECT 1 FROM outbox WHERE case_key=?1 AND lease_owner IS NOT NULL))",
        params![case_key,sql_u64(label_id)?,sql_u64(removal_id)?], |row| row.get(0),
    )?)
}

pub(crate) fn validate(
    transaction: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<u64> {
    let number = |name: &str| {
        input.event.payload[name]
            .as_u64()
            .ok_or(StoreError::InvalidInput("missing reauthorization binding"))
    };
    let revision = number("policy_revision")?;
    if !eligible(
        transaction,
        &current.case_key,
        number("label_event_id")?,
        number("removed_label_event_id")?,
    )? || input.next_state != "PLANNING"
        || input.plan_version != current.plan_version
        || input.remediation_round != current.remediation_round
        || input.pr_number.is_some()
        || input.head_sha.is_some()
        || input.run.is_some()
        || !input.findings.is_empty()
        || !input.evidence.is_empty()
        || input.effects.len() != 1
        || input.effects[0].effect_type != "DISPATCH_PLANNER"
        || input.effects[0].payload
            != serde_json::json!({"case_key":current.case_key,"state_revision":current.state_revision+1,"effect":"DISPATCH_PLANNER"})
    {
        return Err(StoreError::InvalidInput(
            "reauthorization requires withdrawn pre-PR work, fresh label evidence and an unleased generation",
        ));
    }
    let policy: String = transaction.query_row(
        "SELECT payload_json FROM policies WHERE repository_id=?1 AND revision=?2",
        params![sql_u64(current.repository_id)?, sql_u64(revision)?],
        |row| row.get(0),
    )?;
    let policy: Value = serde_json::from_str(&policy)?;
    if policy["intake"]["label"] != input.event.payload["label"]
        || !policy["intake"]["trusted_actor_ids"]
            .as_array()
            .is_some_and(|actors| actors.contains(&input.event.payload["label_actor_id"]))
    {
        return Err(StoreError::InvalidInput(
            "reauthorization label and actor must match the accepted policy",
        ));
    }
    Ok(revision)
}

impl Store {
    /// Resolve a frozen job's policy from its immutable case event, not the
    /// mutable current case projection. Never fall back across generations.
    pub fn accepted_policy_at_case_revision(
        &self,
        case_key: &str,
        state_revision: u64,
    ) -> Result<Value> {
        let (repository_id, policy_revision): (i64, i64) = self.connection.query_row(
            "SELECT c.repository_id, e.policy_revision FROM events e
             JOIN cases c ON c.case_key=e.case_key
             WHERE e.case_key=?1 AND e.state_revision=?2",
            params![case_key, sql_u64(state_revision)?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        self.accepted_policy(unsigned(repository_id), unsigned(policy_revision))
    }

    pub fn can_reauthorize(&self, case_key: &str, label_id: u64, removal_id: u64) -> Result<bool> {
        eligible(&self.connection, case_key, label_id, removal_id)
    }

    pub fn case_task_ids(&self, case_key: &str) -> Result<Vec<String>> {
        let mut statement = self.connection.prepare("SELECT task_id FROM task_projections WHERE case_key=?1 AND task_id IS NOT NULL ORDER BY task_id")?;
        Ok(statement
            .query_map([case_key], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Original creation time is immutable; only a fresh accepted label restarts the age window.
    pub fn case_authorized_at(&self, case_key: &str) -> Result<Option<u64>> {
        self.connection.query_row(
            "SELECT COALESCE((SELECT MAX(observed_at) FROM events WHERE case_key=?1 AND event_type='ISSUE_REAUTHORIZED'),created_at) FROM cases WHERE case_key=?1",
            [case_key], |row| row.get::<_,i64>(0),
        ).optional()?.map(|value| u64::try_from(value).map_err(|_| StoreError::InvalidInteger)).transpose()
    }
}
