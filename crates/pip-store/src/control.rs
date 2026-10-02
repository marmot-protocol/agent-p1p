//! Parked cases and the human commands that resume, replan or abandon them.
use super::*;

pub(crate) const MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS control_commands (
    comment_id INTEGER PRIMARY KEY CHECK (comment_id > 0),
    repository_id INTEGER NOT NULL CHECK (repository_id > 0),
    thread_number INTEGER NOT NULL CHECK (thread_number > 0),
    case_key TEXT REFERENCES cases(case_key),
    actor_id INTEGER NOT NULL CHECK (actor_id > 0),
    command TEXT NOT NULL CHECK (command IN ('RESUME', 'REPLAN', 'ABANDON')),
    guidance TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('PENDING', 'APPLIED', 'IGNORED')),
    outcome TEXT,
    resolved_at INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS control_commands_pending ON control_commands(repository_id, status, received_at);
"#;

const MAX_GUIDANCE_BYTES: usize = 8 * 1024;

/// States in which automation has stopped and waits for a human.
pub const PARKED_STATES: [&str; 3] = ["ESCALATED", "BLOCKED", "WAITING_HUMAN"];

#[derive(Clone, Debug, Serialize)]
pub struct ControlCommandInput {
    pub comment_id: u64,
    pub repository_id: u64,
    pub thread_number: u64,
    pub case_key: Option<String>,
    pub actor_id: u64,
    pub command: String,
    pub guidance: String,
    pub received_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ControlCommandStatus {
    Pending,
    Applied,
    Ignored,
}

impl ControlCommandStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Applied => "APPLIED",
            Self::Ignored => "IGNORED",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControlCommand {
    pub comment_id: u64,
    pub repository_id: u64,
    pub thread_number: u64,
    pub case_key: Option<String>,
    pub actor_id: u64,
    pub command: String,
    pub guidance: String,
    pub received_at: u64,
    pub status: ControlCommandStatus,
}

/// The event that moved a case into its current parked state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParkingEvent {
    pub event_type: String,
    pub previous_state: String,
    pub state_revision: u64,
    pub observed_at: u64,
    pub payload: Value,
}

impl Store {
    pub fn record_control_command(&mut self, input: &ControlCommandInput) -> Result<ApplyResult> {
        self.ensure_writable()?;
        if input.comment_id == 0
            || input.repository_id == 0
            || input.thread_number == 0
            || input.actor_id == 0
            || !matches!(input.command.as_str(), "RESUME" | "REPLAN" | "ABANDON")
            || input.guidance.len() > MAX_GUIDANCE_BYTES
        {
            return Err(StoreError::InvalidInput("invalid control command"));
        }
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO control_commands(comment_id, repository_id, thread_number,
                 case_key, actor_id, command, guidance, received_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'PENDING')",
            params![
                sql_u64(input.comment_id)?,
                sql_u64(input.repository_id)?,
                sql_u64(input.thread_number)?,
                input.case_key,
                sql_u64(input.actor_id)?,
                input.command,
                input.guidance,
                sql_u64(input.received_at)?,
            ],
        )?;
        Ok(if inserted == 1 {
            ApplyResult::Applied
        } else {
            ApplyResult::Replayed
        })
    }

    pub fn pending_control_commands(
        &self,
        repository_id: u64,
        case_key: Option<&str>,
    ) -> Result<Vec<ControlCommand>> {
        let mut statement = self.connection.prepare(
            "SELECT comment_id, repository_id, thread_number, case_key, actor_id, command,
                    guidance, received_at
             FROM control_commands
             WHERE repository_id = ?1 AND status = 'PENDING'
               AND (?2 IS NULL OR case_key = ?2)
             ORDER BY received_at, comment_id",
        )?;
        statement
            .query_map(params![sql_u64(repository_id)?, case_key], |row| {
                Ok(ControlCommand {
                    comment_id: unsigned(row.get(0)?),
                    repository_id: unsigned(row.get(1)?),
                    thread_number: unsigned(row.get(2)?),
                    case_key: row.get(3)?,
                    actor_id: unsigned(row.get(4)?),
                    command: row.get(5)?,
                    guidance: row.get(6)?,
                    received_at: unsigned(row.get(7)?),
                    status: ControlCommandStatus::Pending,
                })
            })?
            .collect::<std::result::Result<_, _>>()
            .map_err(Into::into)
    }

    pub fn resolve_control_command(
        &mut self,
        comment_id: u64,
        status: ControlCommandStatus,
        outcome: &str,
        now: u64,
    ) -> Result<()> {
        self.ensure_writable()?;
        if status == ControlCommandStatus::Pending || outcome.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "a resolved status and outcome are required",
            ));
        }
        let updated = self.connection.execute(
            "UPDATE control_commands SET status = ?1, outcome = ?2, resolved_at = ?3
             WHERE comment_id = ?4 AND status = 'PENDING'",
            params![
                status.as_str(),
                outcome,
                sql_u64(now)?,
                sql_u64(comment_id)?
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::InvalidInput("control command is not pending"));
        }
        Ok(())
    }

    /// The event that parked the case, or `None` when it is not parked.
    pub fn parking_event(&self, case_key: &str) -> Result<Option<ParkingEvent>> {
        let Some(case) = self.case(case_key)? else {
            return Err(StoreError::MissingCase(case_key.into()));
        };
        if !PARKED_STATES.contains(&case.state.as_str()) {
            return Ok(None);
        }
        let row: Option<(String, String, i64, i64, String)> = self
            .connection
            .query_row(
                "SELECT event_type, COALESCE(previous_state, ''), state_revision, observed_at,
                        payload_json
                 FROM events
                 WHERE case_key = ?1 AND next_state = ?2
                   AND previous_state IS NOT next_state
                 ORDER BY state_revision DESC LIMIT 1",
                params![case_key, case.state],
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
            .optional()?;
        row.map(
            |(event_type, previous_state, revision, observed_at, payload)| {
                Ok(ParkingEvent {
                    event_type,
                    previous_state,
                    state_revision: unsigned(revision),
                    observed_at: unsigned(observed_at),
                    payload: serde_json::from_str(&payload)?,
                })
            },
        )
        .transpose()
    }

    /// Remediation rounds humans granted when resuming exhausted cases.
    pub fn granted_remediation_rounds(&self, case_key: &str) -> Result<u32> {
        let value: i64 = self.connection.query_row(
            "SELECT COALESCE(SUM(json_extract(payload_json, '$.granted_rounds')), 0)
             FROM events WHERE case_key = ?1 AND event_type = 'HUMAN_RESUMED'",
            [case_key],
            |row| row.get(0),
        )?;
        u32::try_from(value).map_err(|_| StoreError::InvalidInteger)
    }
}

/// A resume rebinds the case to a recorded policy revision no older than its
/// own: a human explicitly accepted the current settings for this case.
pub(crate) fn validate_resume(
    transaction: &Transaction<'_>,
    current: &StoredCase,
    input: &TransitionInput,
) -> Result<u64> {
    let invalid = || StoreError::InvalidInput("invalid human resume");
    let payload = &input.event.payload;
    let revision = payload["policy_revision"].as_u64().ok_or_else(invalid)?;
    if !PARKED_STATES.contains(&current.state.as_str())
        || revision < current.policy_revision
        || payload["granted_rounds"]
            .as_u64()
            .is_none_or(|rounds| rounds > 10)
    {
        return Err(invalid());
    }
    let known: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM policies WHERE repository_id = ?1 AND revision = ?2)",
        params![sql_u64(current.repository_id)?, sql_u64(revision)?],
        |row| row.get(0),
    )?;
    if !known {
        return Err(invalid());
    }
    Ok(revision)
}
