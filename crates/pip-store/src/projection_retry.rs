//! Retrying one Hermes job without a ledger transition.
//!
//! A rejected result (malformed, crashed, or exhausted inside Hermes) is
//! retried by projecting a fresh Hermes task for the same frozen dispatch
//! effect and state revision. No workflow event is appended, so peer jobs in
//! the same batch (for example the other required reviewer) are unaffected.
use super::*;

/// Desired-payload key that marks a retry projection and names its root.
const RETRY_OF: &str = "pip_retry_of";
const MAX_RETRY_ERROR_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionRejection {
    /// A retry projection was recorded; it becomes due after the cooldown.
    Retry { projection_id: String, attempt: u32 },
    /// The stage's attempt budget is spent; the caller should park the case.
    Exhausted { attempts: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PendingProjection {
    pub projection_id: String,
    pub effect_id: String,
    pub board: String,
    pub desired: Value,
}

impl Store {
    /// Records that a projection's result was rejected and, while the budget
    /// allows, schedules a fresh task for the same job after `cooldown`.
    pub fn reject_task_projection(
        &mut self,
        projection_id: &str,
        now: u64,
        reason: &str,
        max_attempts: u32,
        cooldown: u64,
    ) -> Result<ProjectionRejection> {
        self.ensure_writable()?;
        let reason = bounded(reason.trim());
        if projection_id.trim().is_empty() || reason.is_empty() || max_attempts == 0 {
            return Err(StoreError::InvalidInput(
                "projection, reason, and attempt budget are required",
            ));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (case_key, effect_id, board, task_id, desired): (
            String,
            String,
            String,
            Option<String>,
            String,
        ) = transaction
            .query_row(
                "SELECT case_key, effect_id, board, task_id, desired_json
                 FROM task_projections WHERE projection_id = ?1",
                [projection_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::InvalidInput("unknown task projection"))?;
        let task_id =
            task_id.ok_or_else(|| StoreError::InvalidInput("projection has no task yet"))?;
        let desired: Value = serde_json::from_str(&desired)?;
        let (root, attempt) = match desired.get(RETRY_OF).and_then(Value::as_str) {
            Some(root) => (
                root.to_string(),
                desired["attempt"]
                    .as_u64()
                    .and_then(|attempt| u32::try_from(attempt).ok())
                    .ok_or(StoreError::InvalidInput("invalid retry projection"))?,
            ),
            None => (projection_id.to_string(), 1),
        };
        let next_id = format!("{root}:attempt:{}", attempt + 1);
        let evidence_id = format!("projection-rejected:{projection_id}");
        let already: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM evidence WHERE evidence_id = ?1)",
            [&evidence_id],
            |row| row.get(0),
        )?;
        if !already {
            insert_evidence(
                &transaction,
                &case_key,
                now,
                &[EvidenceInput {
                    evidence_id,
                    kind: "WORKER_RESULT_REJECTED".into(),
                    source: root.clone(),
                    payload: serde_json::json!({
                        "projection_id": projection_id,
                        "task_id": task_id,
                        "attempt": attempt,
                        "reason": reason,
                    }),
                }],
            )?;
            if attempt < max_attempts {
                let retry = serde_json::json!({
                    RETRY_OF: root,
                    "attempt": attempt + 1,
                    "previous_result_error": reason,
                    "not_before": now.checked_add(cooldown).ok_or(StoreError::InvalidInteger)?,
                });
                transaction.execute(
                    "INSERT INTO task_projections(projection_id, case_key, effect_id, board,
                         task_id, desired_json, observed_json, reconciled_at)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6)",
                    params![
                        next_id,
                        case_key,
                        effect_id,
                        board,
                        serde_json::to_string(&retry)?,
                        sql_u64(now)?,
                    ],
                )?;
            }
        }
        let retried: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_projections WHERE projection_id = ?1)",
            [&next_id],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(if retried {
            ProjectionRejection::Retry {
                projection_id: next_id,
                attempt: attempt + 1,
            }
        } else {
            ProjectionRejection::Exhausted { attempts: attempt }
        })
    }

    /// Which attempt at its job a projection is (the original is attempt 1).
    pub fn projection_attempt(&self, projection_id: &str) -> Result<u32> {
        let desired: String = self
            .connection
            .query_row(
                "SELECT desired_json FROM task_projections WHERE projection_id = ?1",
                [projection_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::InvalidInput("unknown task projection"))?;
        let desired: Value = serde_json::from_str(&desired)?;
        if desired.get(RETRY_OF).is_none() {
            return Ok(1);
        }
        desired["attempt"]
            .as_u64()
            .and_then(|attempt| u32::try_from(attempt).ok())
            .ok_or(StoreError::InvalidInput("invalid retry projection"))
    }

    /// Retry projections whose cooldown has passed and that have no task yet.
    pub fn pending_projection_retries(
        &self,
        repository_id: u64,
        case_key: Option<&str>,
        now: u64,
    ) -> Result<Vec<PendingProjection>> {
        let mut statement = self.connection.prepare(
            "SELECT p.projection_id, p.effect_id, p.board, p.desired_json
             FROM task_projections p JOIN cases c ON c.case_key = p.case_key
             WHERE p.task_id IS NULL AND c.repository_id = ?1
               AND (?2 IS NULL OR c.case_key = ?2)
               AND json_extract(p.desired_json, '$.pip_retry_of') IS NOT NULL
               AND json_extract(p.desired_json, '$.not_before') <= ?3
             ORDER BY p.reconciled_at, p.projection_id",
        )?;
        let rows = statement
            .query_map(
                params![sql_u64(repository_id)?, case_key, sql_u64(now)?],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(projection_id, effect_id, board, desired)| {
                Ok(PendingProjection {
                    desired: resolve_projection_desired(
                        &self.connection,
                        &effect_id,
                        serde_json::from_str(&desired)?,
                    )?,
                    projection_id,
                    effect_id,
                    board,
                })
            })
            .collect()
    }

    /// Binds a created Hermes task to its retry projection. Replays are safe.
    pub fn record_projection_task(
        &mut self,
        projection_id: &str,
        task_id: &str,
        observed: &Value,
        now: u64,
    ) -> Result<()> {
        self.ensure_writable()?;
        if projection_id.trim().is_empty() || task_id.trim().is_empty() {
            return Err(StoreError::InvalidInput("projection and task are required"));
        }
        let updated = self.connection.execute(
            "UPDATE task_projections SET task_id = ?1, observed_json = ?2, reconciled_at = ?3
             WHERE projection_id = ?4 AND task_id IS NULL",
            params![
                task_id,
                serde_json::to_string(observed)?,
                sql_u64(now)?,
                projection_id
            ],
        )?;
        if updated == 1 {
            return Ok(());
        }
        let existing: Option<Option<String>> = self
            .connection
            .query_row(
                "SELECT task_id FROM task_projections WHERE projection_id = ?1",
                [projection_id],
                |row| row.get(0),
            )
            .optional()?;
        match existing {
            Some(Some(existing)) if existing == task_id => Ok(()),
            Some(_) => Err(StoreError::IdempotencyConflict {
                id: format!("projection:{projection_id}"),
            }),
            None => Err(StoreError::InvalidInput("unknown task projection")),
        }
    }
}

/// Resolves a stored projection payload to its full desired task. A retry
/// projection is its root job with a distinct projection key, a distinct
/// Hermes idempotency key, and the previous attempt's error in the body.
pub(crate) fn resolve_projection_desired(
    connection: &Connection,
    effect_id: &str,
    desired: Value,
) -> Result<Value> {
    let Some(root) = desired.get(RETRY_OF).and_then(Value::as_str) else {
        return dispatch_intents::resolve_output(
            connection,
            effect_id,
            DispatchTransport::Hermes,
            desired,
        );
    };
    let invalid = || StoreError::InvalidInput("invalid retry projection");
    let attempt = desired["attempt"].as_u64().ok_or_else(invalid)?;
    let error = desired["previous_result_error"]
        .as_str()
        .ok_or_else(invalid)?;
    let (root_effect, root_desired): (String, String) = connection
        .query_row(
            "SELECT effect_id, desired_json FROM task_projections WHERE projection_id = ?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(invalid)?;
    let root_desired: Value = serde_json::from_str(&root_desired)?;
    if root_effect != effect_id || root_desired.get(RETRY_OF).is_some() {
        return Err(invalid());
    }
    let mut resolved = dispatch_intents::resolve_output(
        connection,
        effect_id,
        DispatchTransport::Hermes,
        root_desired,
    )?;
    let object = resolved.as_object_mut().ok_or_else(invalid)?;
    for key in ["projection_key", "effect_id"] {
        let value = object
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        let retried = format!("{value}:attempt:{attempt}");
        object.insert(key.into(), Value::String(retried));
    }
    object
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid)?
        .insert("previous_result_error".into(), Value::String(error.into()));
    Ok(resolved)
}

fn bounded(text: &str) -> String {
    let mut end = text.len().min(MAX_RETRY_ERROR_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
