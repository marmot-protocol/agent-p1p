//! Bounded, read-only operational evidence for the controller.
//! Event payloads must be whitelisted before exposing them in worker tasks.

use crate::{Result, Store, StoredEvent, unsigned};
use serde_json::{Value, json};

pub struct CaseActivity {
    pub recent_events: Vec<StoredEvent>,
    pub recent_attempts: Vec<Value>,
    pub pending_effect_count: u64,
}

impl Store {
    /// Newest first, at most eight records per history. No leases are acquired.
    pub fn case_activity(&self, case_key: &str) -> Result<CaseActivity> {
        let mut events = self.connection.prepare(
            "SELECT event_id, state_revision, observed_at, event_type, payload_json,
                    payload_sha256 FROM events WHERE case_key = ?1
             ORDER BY state_revision DESC LIMIT 8",
        )?;
        let recent_events = events
            .query_map([case_key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .map(|row| {
                let (event_id, revision, observed_at, event_type, payload, payload_sha256) = row?;
                Ok(StoredEvent {
                    event_id,
                    state_revision: unsigned(revision),
                    observed_at: unsigned(observed_at),
                    event_type,
                    payload: serde_json::from_str(&payload)?,
                    payload_sha256,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut attempts = self.connection.prepare(
            "SELECT attempt_id, state_revision, status, started_at, completed_at
             FROM direct_attempts WHERE case_key = ?1 ORDER BY attempt_id DESC LIMIT 8",
        )?;
        let recent_attempts = attempts
            .query_map([case_key], |row| {
                Ok(json!({"attempt_id":row.get::<_, i64>(0)?,
                "state_revision":row.get::<_, i64>(1)?, "status":row.get::<_, String>(2)?,
                "started_at":row.get::<_, i64>(3)?, "completed_at":row.get::<_, Option<i64>>(4)?}))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let pending: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM outbox WHERE case_key = ?1
             AND delivered_at IS NULL AND superseded_at IS NULL",
            [case_key],
            |row| row.get(0),
        )?;
        Ok(CaseActivity {
            recent_events,
            recent_attempts,
            pending_effect_count: unsigned(pending),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectInput, EventInput, NewCase};

    #[test]
    fn activity_is_case_scoped_bounded_read_only_and_omits_worker_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.db");
        let mut store = Store::open(&path).unwrap();
        for key in ["target", "unrelated"] {
            store
                .create_case(&NewCase {
                    case_key: key.into(),
                    repository_id: 1,
                    issue_number: if key == "target" { 1 } else { 2 },
                    workflow_version: 1,
                    policy_revision: 1,
                    initial_state: "ESCALATED".into(),
                    observed_at: 1,
                    event: EventInput {
                        event_id: key.into(),
                        event_type: "TEST".into(),
                        payload: json!({}),
                    },
                    effects: vec![EffectInput {
                        effect_id: key.into(),
                        effect_type: "TEST".into(),
                        payload: json!({}),
                    }],
                })
                .unwrap();
            for revision in 2..=12 {
                store.connection.execute(
                    "INSERT INTO events SELECT ?1, case_key, ?2, ?2, event_type, payload_json,
                     payload_sha256, command_sha256, previous_state, next_state, policy_revision,
                     remediation_round, plan_version, pr_number, head_sha FROM events WHERE event_id=?3",
                    rusqlite::params![format!("{key}-{revision}"),revision,key],
                ).unwrap();
                store.connection.execute(
                    "INSERT INTO direct_attempts (effect_id,case_key,state_revision,task_id,lease_owner,
                     lease_until,started_at,completed_at,status,error)
                     VALUES (?1,?1,?2,'task','private-owner',100,1,2,'FAILED','private-error')",
                    rusqlite::params![key,revision],
                ).unwrap();
            }
        }
        drop(store);
        let store = Store::open_read_only(&path).unwrap();
        let activity = store.case_activity("target").unwrap();
        assert_eq!(activity.recent_events.len(), 8);
        assert_eq!(activity.recent_events[0].state_revision, 12);
        assert!(
            activity
                .recent_events
                .iter()
                .all(|event| event.event_id.starts_with("target-"))
        );
        assert_eq!(activity.recent_attempts.len(), 8);
        assert_eq!(activity.recent_attempts[0]["state_revision"], 12);
        assert_eq!(activity.recent_attempts[0]["attempt_id"], 11);
        assert!(
            !serde_json::to_string(&activity.recent_attempts)
                .unwrap()
                .contains("private")
        );
        assert_eq!(activity.pending_effect_count, 1);
        assert!(
            store
                .case_activity("missing")
                .unwrap()
                .recent_events
                .is_empty()
        );
    }
}
