//! Durable GitHub conversations, deliberately separate from build authorization.
use super::*;

pub(crate) const MIGRATION: &str = r#"
CREATE TABLE conversations (
    message_key TEXT PRIMARY KEY,
    repository_id INTEGER NOT NULL,
    thread_number INTEGER NOT NULL,
    actor_id INTEGER NOT NULL,
    case_key TEXT REFERENCES cases(case_key),
    received_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    payload_sha256 TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'PENDING' CHECK(state IN ('PENDING','CREATING','QUEUED','ANSWERED','PUBLISHED','IGNORED','FAILED')),
    spec_json TEXT CHECK(spec_json IS NULL OR json_valid(spec_json)),
    task_id TEXT UNIQUE,
    answer_json TEXT CHECK(answer_json IS NULL OR json_valid(answer_json)),
    reply_id INTEGER,
    reply_body TEXT
) STRICT;
CREATE INDEX conversations_pending ON conversations(repository_id,state,received_at,message_key);
CREATE TRIGGER conversations_input_immutable BEFORE UPDATE ON conversations
WHEN NEW.message_key != OLD.message_key OR NEW.repository_id != OLD.repository_id
 OR NEW.thread_number != OLD.thread_number OR NEW.actor_id != OLD.actor_id
 OR NEW.case_key IS NOT OLD.case_key OR NEW.received_at != OLD.received_at
 OR NEW.payload_json != OLD.payload_json OR NEW.payload_sha256 != OLD.payload_sha256
 OR (OLD.spec_json IS NOT NULL AND NEW.spec_json IS NOT OLD.spec_json)
 OR (OLD.task_id IS NOT NULL AND NEW.task_id IS NOT OLD.task_id)
 OR (OLD.answer_json IS NOT NULL AND NEW.answer_json IS NOT OLD.answer_json)
 OR (OLD.reply_id IS NOT NULL AND NEW.reply_id IS NOT OLD.reply_id)
 OR (OLD.reply_body IS NOT NULL AND NEW.reply_body IS NOT OLD.reply_body)
BEGIN SELECT RAISE(ABORT, 'conversation evidence is immutable'); END;
CREATE TRIGGER conversations_no_delete BEFORE DELETE ON conversations
BEGIN SELECT RAISE(ABORT, 'conversations are retained'); END;
"#;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ConversationInput {
    pub key: String,
    pub repository_id: u64,
    pub thread_number: u64,
    pub actor_id: u64,
    pub case_key: Option<String>,
    pub received_at: u64,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct Conversation {
    pub input: ConversationInput,
    pub state: String,
    pub spec: Option<Value>,
    pub task_id: Option<String>,
    pub answer: Option<Value>,
    pub reply_body: Option<String>,
}

impl Store {
    pub fn record_conversation(&mut self, input: &ConversationInput) -> Result<ApplyResult> {
        self.ensure_writable()?;
        if !valid_identifier(&input.key, 256)
            || input.repository_id == 0
            || input.thread_number == 0
            || input.actor_id == 0
            || input.received_at == 0
            || !input.payload.is_object()
            || serde_json::to_vec(&input.payload)?.len() > 256 * 1024
        {
            return Err(StoreError::InvalidInput("invalid conversation"));
        }
        if let Some(existing) = self.conversation(&input.key)? {
            let mut replay = input.clone();
            replay.received_at = existing.input.received_at;
            return if existing.input == replay {
                Ok(ApplyResult::Replayed)
            } else {
                Err(StoreError::IdempotencyConflict {
                    id: input.key.clone(),
                })
            };
        }
        let (body, hash) = payload(&input.payload)?;
        self.connection.execute("INSERT INTO conversations(message_key,repository_id,thread_number,actor_id,case_key,received_at,payload_json,payload_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![input.key,sql_u64(input.repository_id)?,sql_u64(input.thread_number)?,sql_u64(input.actor_id)?,input.case_key,sql_u64(input.received_at)?,body,hash])?;
        Ok(ApplyResult::Applied)
    }

    pub fn conversations(&self, repository_id: u64, limit: u32) -> Result<Vec<Conversation>> {
        let mut statement = self.connection.prepare("SELECT message_key FROM conversations WHERE repository_id=?1 AND state NOT IN ('PUBLISHED','IGNORED','FAILED') ORDER BY received_at,message_key LIMIT ?2")?;
        let keys = statement
            .query_map(params![sql_u64(repository_id)?, limit.min(100)], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        keys.iter()
            .map(|key| {
                self.conversation(key)?
                    .ok_or(StoreError::InvalidInput("missing conversation"))
            })
            .collect()
    }

    pub fn has_pending_conversation(
        &self,
        repository_id: u64,
        case_key: Option<&str>,
    ) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE repository_id=?1
             AND (?2 IS NULL OR case_key=?2) AND state NOT IN ('PUBLISHED','IGNORED','FAILED'))",
            params![sql_u64(repository_id)?, case_key],
            |row| row.get(0),
        )?)
    }

    pub fn conversation(&self, key: &str) -> Result<Option<Conversation>> {
        let raw = self.connection.query_row("SELECT repository_id,thread_number,actor_id,case_key,received_at,payload_json,payload_sha256,state,spec_json,task_id,answer_json,reply_body FROM conversations WHERE message_key=?1", [key], |row| {
            Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,i64>(4)?,row.get::<_,String>(5)?,row.get::<_,String>(6)?,row.get::<_,String>(7)?,row.get::<_,Option<String>>(8)?,row.get::<_,Option<String>>(9)?,row.get::<_,Option<String>>(10)?,row.get::<_,Option<String>>(11)?))
        }).optional()?;
        raw.map(|r| {
            let value: Value = serde_json::from_str(&r.5)?;
            if payload(&value)?.1 != r.6 {
                return Err(StoreError::InvalidInput("conversation digest differs"));
            }
            Ok(Conversation {
                input: ConversationInput {
                    key: key.into(),
                    repository_id: unsigned(r.0),
                    thread_number: unsigned(r.1),
                    actor_id: unsigned(r.2),
                    case_key: r.3,
                    received_at: unsigned(r.4),
                    payload: value,
                },
                state: r.7,
                spec: r.8.map(|s| serde_json::from_str(&s)).transpose()?,
                task_id: r.9,
                answer: r.10.map(|s| serde_json::from_str(&s)).transpose()?,
                reply_body: r.11,
            })
        })
        .transpose()
    }

    /// Reserve once before invoking an external queue. An uncertain create is
    /// reconciled read-only and never automatically issued a second time.
    pub fn reserve_conversation(&mut self, key: &str, spec: &Value) -> Result<bool> {
        self.ensure_writable()?;
        if !spec.is_object() || serde_json::to_vec(spec)?.len() > 512 * 1024 {
            return Err(StoreError::InvalidInput("invalid conversation job"));
        }
        Ok(self.connection.execute("UPDATE conversations SET state='CREATING',spec_json=?2 WHERE message_key=?1 AND state='PENDING'", params![key,payload(spec)?.0])? == 1)
    }

    pub fn bind_conversation_task(&mut self, key: &str, task: &str) -> Result<()> {
        self.ensure_writable()?;
        if !valid_identifier(task, 256) {
            return Err(StoreError::InvalidInput("invalid task"));
        }
        let changed = self.connection.execute("UPDATE conversations SET state='QUEUED',task_id=?2 WHERE message_key=?1 AND state='CREATING'",params![key,task])?;
        if changed == 1
            || self
                .conversation(key)?
                .is_some_and(|c| c.task_id.as_deref() == Some(task))
        {
            Ok(())
        } else {
            Err(StoreError::IdempotencyConflict { id: key.into() })
        }
    }

    pub fn answer_conversation(&mut self, key: &str, answer: &Value) -> Result<()> {
        self.ensure_writable()?;
        if !answer.is_object() || serde_json::to_vec(answer)?.len() > 16 * 1024 {
            return Err(StoreError::InvalidInput("invalid conversation answer"));
        }
        let changed = self.connection.execute("UPDATE conversations SET state='ANSWERED',answer_json=?2 WHERE message_key=?1 AND state='QUEUED'",params![key,payload(answer)?.0])?;
        if changed == 1
            || self
                .conversation(key)?
                .is_some_and(|c| c.answer.as_ref() == Some(answer))
        {
            Ok(())
        } else {
            Err(StoreError::IdempotencyConflict { id: key.into() })
        }
    }

    pub fn finish_conversation(
        &mut self,
        key: &str,
        state: &str,
        reply_id: Option<u64>,
    ) -> Result<()> {
        self.ensure_writable()?;
        if !matches!(state, "PUBLISHED" | "IGNORED" | "FAILED")
            || (state == "PUBLISHED" && reply_id.is_none_or(|id| id == 0))
        {
            return Err(StoreError::InvalidInput("invalid conversation disposition"));
        }
        let changed = self.connection.execute("UPDATE conversations SET state=?2,reply_id=?3 WHERE message_key=?1 AND state NOT IN ('PUBLISHED','IGNORED','FAILED') AND (?2!='PUBLISHED' OR state='ANSWERED')",params![key,state,reply_id.map(sql_u64).transpose()?])?;
        let replay = self
            .connection
            .query_row(
                "SELECT state=?2 AND reply_id IS ?3 FROM conversations WHERE message_key=?1",
                params![key, state, reply_id.map(sql_u64).transpose()?],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if changed == 1 || replay {
            Ok(())
        } else {
            Err(StoreError::IdempotencyConflict { id: key.into() })
        }
    }

    /// Freeze the rendered publication before any GitHub write. A retry uses
    /// these exact bytes even if authorization or workflow state later changes.
    pub fn prepare_conversation_reply(&mut self, key: &str, body: &str) -> Result<()> {
        self.ensure_writable()?;
        if body.trim().is_empty() || body.len() > 16 * 1024 {
            return Err(StoreError::InvalidInput("invalid reply"));
        }
        let changed = self.connection.execute("UPDATE conversations SET reply_body=?2 WHERE message_key=?1 AND state='ANSWERED' AND reply_body IS NULL",params![key,body])?;
        if changed == 1
            || self
                .conversation(key)?
                .is_some_and(|c| c.reply_body.as_deref() == Some(body))
        {
            Ok(())
        } else {
            Err(StoreError::IdempotencyConflict { id: key.into() })
        }
    }
}
