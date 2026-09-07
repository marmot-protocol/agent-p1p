//! Frozen dispatch batches and fail-closed external create reservations.
//!
//! A reservation means a create MAY have happened, not that it succeeded. It is
//! never reissued, including after lease expiry. Recovery must reconcile the
//! external task against the frozen intent; absence requires operator attention.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ApplyResult, ClaimedEffect, Result, Store, StoreError, payload, sql_u64};

pub(crate) const MIGRATION: &str = r#"
CREATE TABLE dispatch_batches (
    effect_id TEXT PRIMARY KEY REFERENCES outbox(effect_id),
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    state_revision INTEGER NOT NULL CHECK (state_revision > 0),
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    frozen_at INTEGER NOT NULL
) STRICT;

CREATE TABLE dispatch_create_attempts (
    effect_id TEXT NOT NULL REFERENCES dispatch_batches(effect_id),
    intent_id TEXT NOT NULL UNIQUE,
    lease_owner TEXT NOT NULL,
    lease_until INTEGER NOT NULL,
    attempted_at INTEGER NOT NULL,
    PRIMARY KEY (effect_id, intent_id)
) STRICT;

CREATE TRIGGER dispatch_batches_no_update
BEFORE UPDATE ON dispatch_batches BEGIN
    SELECT RAISE(ABORT, 'dispatch batches are immutable');
END;
CREATE TRIGGER dispatch_batches_no_delete
BEFORE DELETE ON dispatch_batches BEGIN
    SELECT RAISE(ABORT, 'dispatch batches are immutable');
END;
CREATE TRIGGER dispatch_create_attempts_no_update
BEFORE UPDATE ON dispatch_create_attempts BEGIN
    SELECT RAISE(ABORT, 'dispatch create attempts are immutable');
END;
CREATE TRIGGER dispatch_create_attempts_no_delete
BEFORE DELETE ON dispatch_create_attempts BEGIN
    SELECT RAISE(ABORT, 'dispatch create attempts are immutable');
END;
"#;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchTransport {
    Hermes,
    Direct,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchIntent {
    pub intent_id: String,
    pub transport: DispatchTransport,
    pub desired: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FrozenBatch {
    format: u32,
    intents: Vec<DispatchIntent>,
    evidence: BTreeMap<String, Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IntentReference {
    schema_version: u32,
    effect_id: String,
    intent_id: String,
    sha256: String,
}

const REFERENCE: &str = "dispatch_intent_ref";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateReservation {
    /// Persisted before invoking the external create command. Use once only.
    Granted,
    /// A previous caller may still be running or may have lost its reply.
    /// Reconcile an existing task; never issue another create for this intent.
    Uncertain,
}

impl Store {
    /// Freeze the complete resolved batch, including reviewer fan-out membership,
    /// before ANY queue write. Replays must match every intent, not just its key.
    pub fn freeze_dispatch_intents(
        &mut self,
        claimed: &ClaimedEffect,
        intents: &[DispatchIntent],
        now: u64,
    ) -> Result<ApplyResult> {
        self.ensure_writable()?;
        let unique = intents
            .iter()
            .map(|intent| &intent.intent_id)
            .collect::<BTreeSet<_>>();
        if intents.is_empty()
            || unique.len() != intents.len()
            || intents.iter().any(|intent| {
                intent.intent_id.trim().is_empty()
                    || intent.intent_id.len() > 512
                    || !intent.desired.is_object()
            })
        {
            return Err(StoreError::InvalidInput(
                "unique nonempty dispatch intents with object payloads are required",
            ));
        }
        // Ordering is not identity: callers may enumerate reviewers differently.
        let mut ordered = intents.to_vec();
        ordered.sort_by(|left, right| left.intent_id.cmp(&right.intent_id));
        let (json, hash) = payload(&freeze_inputs(&ordered)?)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_dispatch_claim(&transaction, claimed, now)?;
        if let Some(existing) = read_intents(&transaction, &claimed.effect_id)? {
            if existing != ordered {
                return Err(StoreError::IdempotencyConflict {
                    id: claimed.effect_id.clone(),
                });
            }
            transaction.commit()?;
            return Ok(ApplyResult::Replayed);
        }
        transaction.execute(
            "INSERT INTO dispatch_batches(effect_id, case_key, state_revision, payload_json, payload_sha256, frozen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![claimed.effect_id, claimed.case_key, sql_u64(claimed.state_revision)?, json, hash, sql_u64(now)?],
        )?;
        transaction.commit()?;
        Ok(ApplyResult::Applied)
    }

    /// Read-only recovery access, including superseded/abandoned dispatches.
    pub fn dispatch_intents(&self, effect_id: &str) -> Result<Option<Vec<DispatchIntent>>> {
        read_intents(&self.connection, effect_id)
    }

    /// Atomically grant at most one create attempt per Hermes intent. No lease
    /// expiration, subprocess error, or restart resets this reservation.
    pub fn reserve_dispatch_create(
        &mut self,
        claimed: &ClaimedEffect,
        intent_id: &str,
        now: u64,
    ) -> Result<CreateReservation> {
        self.ensure_writable()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_dispatch_claim(&transaction, claimed, now)?;
        let intents = read_intents(&transaction, &claimed.effect_id)?;
        if !intents.as_ref().is_some_and(|intents| {
            intents.iter().any(|intent| {
                intent.intent_id == intent_id && intent.transport == DispatchTransport::Hermes
            })
        }) {
            return Err(StoreError::InvalidInput(
                "create reservation requires a frozen Hermes intent",
            ));
        }
        let inserted = transaction.execute(
            "INSERT INTO dispatch_create_attempts(effect_id, intent_id, lease_owner, lease_until, attempted_at)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(intent_id) DO NOTHING",
            params![claimed.effect_id, intent_id, claimed.lease_owner, sql_u64(claimed.lease_until)?, sql_u64(now)?],
        )?;
        transaction.commit()?;
        Ok(if inserted == 1 {
            CreateReservation::Granted
        } else {
            CreateReservation::Uncertain
        })
    }
}

fn read_intents(connection: &Connection, effect_id: &str) -> Result<Option<Vec<DispatchIntent>>> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT payload_json, payload_sha256 FROM dispatch_batches WHERE effect_id = ?1",
            [effect_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(json, hash)| {
        let value: Value = serde_json::from_str(&json)?;
        if payload(&value)?.1 != hash {
            return Err(StoreError::IdempotencyConflict {
                id: effect_id.to_owned(),
            });
        }
        thaw_inputs(value)
    })
    .transpose()
}

fn freeze_inputs(intents: &[DispatchIntent]) -> Result<Value> {
    let mut batch = FrozenBatch {
        format: 1,
        intents: intents.to_vec(),
        evidence: BTreeMap::new(),
    };
    for intent in &mut batch.intents {
        if let Some(bundle) = intent
            .desired
            .get_mut("body")
            .and_then(|body| body.get_mut("immutable_evidence_bundle"))
        {
            if !bundle.is_object() {
                return Err(StoreError::InvalidInput(
                    "dispatch evidence must be an object",
                ));
            }
            let digest = payload(bundle)?.1;
            batch.evidence.insert(digest.clone(), bundle.take());
            *bundle = json!({"$ref":digest});
        }
    }
    Ok(serde_json::to_value(batch)?)
}

fn thaw_inputs(value: Value) -> Result<Vec<DispatchIntent>> {
    // Existing arrays are immutable historical definitions, not migration targets.
    if value.is_array() {
        return Ok(serde_json::from_value(value)?);
    }
    let mut batch: FrozenBatch = serde_json::from_value(value)?;
    if batch.format != 1 {
        return Err(StoreError::InvalidInput(
            "unsupported frozen dispatch format",
        ));
    }
    for (digest, bundle) in &batch.evidence {
        if !bundle.is_object() || payload(bundle)?.1 != *digest {
            return Err(StoreError::InvalidInput(
                "frozen dispatch evidence digest differs",
            ));
        }
    }
    for intent in &mut batch.intents {
        if let Some(bundle) = intent
            .desired
            .get_mut("body")
            .and_then(|body| body.get_mut("immutable_evidence_bundle"))
        {
            let digest = bundle
                .get("$ref")
                .and_then(Value::as_str)
                .filter(|_| bundle.as_object().is_some_and(|object| object.len() == 1))
                .ok_or(StoreError::InvalidInput(
                    "invalid frozen evidence reference",
                ))?;
            *bundle = batch
                .evidence
                .get(digest)
                .ok_or(StoreError::InvalidInput("missing frozen dispatch evidence"))?
                .clone();
        }
    }
    Ok(batch.intents)
}

pub(crate) fn encode_output(
    connection: &Connection,
    effect_id: &str,
    transport: DispatchTransport,
    desired: &Value,
) -> Result<Value> {
    if desired.get(REFERENCE).is_some() {
        return Err(StoreError::InvalidInput(
            "dispatch outputs must contain resolved inputs",
        ));
    }
    let Some(intents) = read_intents(connection, effect_id)? else {
        // Pre-freezing historical dispatches remain readable and replayable.
        return Ok(desired.clone());
    };
    let intent = intents
        .iter()
        .find(|intent| intent.transport == transport && intent.desired == *desired)
        .ok_or_else(|| StoreError::IdempotencyConflict {
            id: effect_id.into(),
        })?;
    Ok(json!({REFERENCE: IntentReference {
        schema_version: 1, effect_id: effect_id.into(), intent_id: intent.intent_id.clone(),
        sha256: payload(desired)?.1,
    }}))
}

pub(crate) fn resolve_output(
    connection: &Connection,
    output_effect_id: &str,
    transport: DispatchTransport,
    value: Value,
) -> Result<Value> {
    let Some(reference) = value.get(REFERENCE) else {
        return Ok(value);
    };
    let reference: IntentReference = serde_json::from_value(reference.clone())?;
    let valid = value.as_object().is_some_and(|object| object.len() == 1)
        && reference.schema_version == 1
        && (transport != DispatchTransport::Hermes || reference.effect_id == output_effect_id);
    let same_case: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM dispatch_batches b JOIN outbox o ON o.case_key=b.case_key
         WHERE b.effect_id=?1 AND o.effect_id=?2)",
        params![reference.effect_id, output_effect_id],
        |row| row.get(0),
    )?;
    if !valid || !same_case {
        return Err(StoreError::InvalidInput(
            "foreign or invalid frozen dispatch reference",
        ));
    }
    let desired = read_intents(connection, &reference.effect_id)?
        .and_then(|intents| {
            intents.into_iter().find(|intent| {
                intent.intent_id == reference.intent_id && intent.transport == transport
            })
        })
        .ok_or(StoreError::InvalidInput("missing frozen dispatch intent"))?
        .desired;
    if payload(&desired)?.1 != reference.sha256 {
        return Err(StoreError::InvalidInput(
            "frozen dispatch intent digest differs",
        ));
    }
    Ok(desired)
}

fn require_dispatch_claim(
    connection: &Connection,
    claimed: &ClaimedEffect,
    now: u64,
) -> Result<()> {
    let valid: bool = connection.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM outbox o JOIN cases c ON c.case_key = o.case_key
             WHERE o.effect_id = ?1 AND o.case_key = ?2 AND o.state_revision = ?3
               AND o.effect_type = ?4 AND o.lease_owner = ?5 AND o.lease_until = ?6
               AND o.lease_until >= ?7 AND o.delivered_at IS NULL AND o.superseded_at IS NULL
               AND c.state_revision = o.state_revision
               AND c.state NOT IN ('ABANDONED', 'BLOCKED', 'ESCALATED', 'COMPLETED', 'TAKEN_OVER')
               AND o.effect_type IN ('DISPATCH_PLANNER', 'DISPATCH_BUILDER', 'DISPATCH_REVIEWERS', 'DISPATCH_FINAL_REVIEWER')
         )",
        params![claimed.effect_id, claimed.case_key, sql_u64(claimed.state_revision)?, claimed.effect_type,
                claimed.lease_owner, sql_u64(claimed.lease_until)?, sql_u64(now)?],
        |row| row.get(0),
    )?;
    if !valid {
        return Err(StoreError::LeaseLost(claimed.effect_id.clone()));
    }
    Ok(())
}
