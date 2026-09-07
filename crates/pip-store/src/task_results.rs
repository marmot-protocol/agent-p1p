//! Retained native completions are evidence, not accepted runs or new jobs.
use super::*;

impl Store {
    pub fn retained_task_result(&self, task_id: &str) -> Result<Option<Value>> {
        let row: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT e.payload_json,e.payload_sha256 FROM evidence e
             JOIN task_projections p ON p.projection_id=e.source
             WHERE p.task_id=?1 AND e.kind='HERMES_RESULT' AND e.evidence_id=?2",
                params![task_id, format!("hermes-result-{task_id}")],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(json, hash)| {
            let value: Value = serde_json::from_str(&json)?;
            if payload(&value)?.1 != hash {
                return Err(StoreError::InvalidInput(
                    "retained task result digest differs",
                ));
            }
            Ok(value)
        })
        .transpose()
    }

    /// Caller validates the worker contract and immutable task binding first.
    /// Repeated collection is idempotent; conflicting completions never overwrite.
    pub fn retain_task_result(
        &mut self,
        task_id: &str,
        value: &Value,
        now: u64,
    ) -> Result<ApplyResult> {
        self.ensure_writable()?;
        if !value.is_object() || serde_json::to_vec(value)?.len() > 4 * 1024 * 1024 {
            return Err(StoreError::InvalidInput(
                "task result must be a bounded object",
            ));
        }
        if let Some(existing) = self.retained_task_result(task_id)? {
            return if existing == *value {
                Ok(ApplyResult::Replayed)
            } else {
                Err(StoreError::IdempotencyConflict { id: task_id.into() })
            };
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (case_key, projection_id): (String, String) = transaction.query_row(
            "SELECT o.case_key,p.projection_id FROM task_projections p
             JOIN outbox o ON o.effect_id=p.effect_id WHERE p.task_id=?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        insert_evidence(
            &transaction,
            &case_key,
            now,
            &[EvidenceInput {
                evidence_id: format!("hermes-result-{task_id}"),
                kind: "HERMES_RESULT".into(),
                source: projection_id,
                payload: value.clone(),
            }],
        )?;
        transaction.commit()?;
        Ok(ApplyResult::Applied)
    }
}
