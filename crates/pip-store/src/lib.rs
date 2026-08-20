//! Authoritative SQLite ledger and transactional outbox.

#![forbid(unsafe_code)]

use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{
    Connection, MAIN_DB, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u32 = 3;

const MIGRATION_1: &str = r#"
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
) STRICT;

CREATE TABLE policies (
    repository_id INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    accepted_at INTEGER NOT NULL,
    PRIMARY KEY (repository_id, revision)
) STRICT;

CREATE TABLE cases (
    case_key TEXT PRIMARY KEY,
    repository_id INTEGER NOT NULL,
    issue_number INTEGER NOT NULL,
    workflow_version INTEGER NOT NULL,
    state TEXT NOT NULL,
    state_revision INTEGER NOT NULL CHECK (state_revision > 0),
    policy_revision INTEGER NOT NULL CHECK (policy_revision > 0),
    remediation_round INTEGER NOT NULL DEFAULT 0 CHECK (remediation_round >= 0),
    plan_version INTEGER NOT NULL DEFAULT 0 CHECK (plan_version >= 0),
    pr_number INTEGER,
    head_sha TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (repository_id, issue_number, workflow_version)
) STRICT;

CREATE TABLE events (
    event_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    state_revision INTEGER NOT NULL CHECK (state_revision > 0),
    observed_at INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    command_sha256 TEXT NOT NULL,
    previous_state TEXT,
    next_state TEXT NOT NULL,
    policy_revision INTEGER NOT NULL CHECK (policy_revision > 0),
    remediation_round INTEGER NOT NULL CHECK (remediation_round >= 0),
    plan_version INTEGER NOT NULL CHECK (plan_version >= 0),
    pr_number INTEGER,
    head_sha TEXT,
    UNIQUE (case_key, state_revision)
) STRICT;

CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    task_id TEXT NOT NULL UNIQUE,
    role TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    accepted_at INTEGER NOT NULL
) STRICT;

CREATE TABLE evidence (
    evidence_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    observed_at INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL
) STRICT;

CREATE TABLE findings (
    finding_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    origin_role TEXT NOT NULL,
    reviewed_head_sha TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    recorded_at INTEGER NOT NULL
) STRICT;

CREATE TABLE outbox (
    effect_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    state_revision INTEGER NOT NULL CHECK (state_revision > 0),
    effect_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    lease_owner TEXT,
    lease_until INTEGER,
    delivered_at INTEGER,
    CHECK ((lease_owner IS NULL) = (lease_until IS NULL))
) STRICT;

CREATE TABLE task_projections (
    projection_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    effect_id TEXT NOT NULL UNIQUE REFERENCES outbox(effect_id),
    board TEXT NOT NULL,
    task_id TEXT,
    desired_json TEXT NOT NULL,
    observed_json TEXT,
    reconciled_at INTEGER
) STRICT;

CREATE INDEX outbox_ready ON outbox(delivered_at, lease_until, created_at);
CREATE INDEX events_case_revision ON events(case_key, state_revision);

CREATE TRIGGER policies_no_update BEFORE UPDATE ON policies BEGIN
    SELECT RAISE(ABORT, 'policies are immutable');
END;
CREATE TRIGGER policies_no_delete BEFORE DELETE ON policies BEGIN
    SELECT RAISE(ABORT, 'policies are immutable');
END;
CREATE TRIGGER events_no_update BEFORE UPDATE ON events BEGIN
    SELECT RAISE(ABORT, 'events are immutable');
END;
CREATE TRIGGER events_no_delete BEFORE DELETE ON events BEGIN
    SELECT RAISE(ABORT, 'events are immutable');
END;
CREATE TRIGGER runs_no_update BEFORE UPDATE ON runs BEGIN
    SELECT RAISE(ABORT, 'runs are immutable');
END;
CREATE TRIGGER runs_no_delete BEFORE DELETE ON runs BEGIN
    SELECT RAISE(ABORT, 'runs are immutable');
END;
CREATE TRIGGER evidence_no_update BEFORE UPDATE ON evidence BEGIN
    SELECT RAISE(ABORT, 'evidence is immutable');
END;
CREATE TRIGGER evidence_no_delete BEFORE DELETE ON evidence BEGIN
    SELECT RAISE(ABORT, 'evidence is immutable');
END;
CREATE TRIGGER findings_no_update BEFORE UPDATE ON findings BEGIN
    SELECT RAISE(ABORT, 'findings are immutable');
END;
CREATE TRIGGER findings_no_delete BEFORE DELETE ON findings BEGIN
    SELECT RAISE(ABORT, 'findings are immutable');
END;
"#;

const MIGRATION_2: &str = r#"
ALTER TABLE task_projections RENAME TO task_projections_v1;

CREATE TABLE task_projections (
    projection_id TEXT PRIMARY KEY,
    case_key TEXT NOT NULL REFERENCES cases(case_key),
    effect_id TEXT NOT NULL REFERENCES outbox(effect_id),
    board TEXT NOT NULL,
    task_id TEXT,
    desired_json TEXT NOT NULL,
    observed_json TEXT,
    reconciled_at INTEGER
) STRICT;

INSERT INTO task_projections(
    projection_id, case_key, effect_id, board, task_id,
    desired_json, observed_json, reconciled_at
)
SELECT
    projection_id, case_key, effect_id, board, task_id,
    desired_json, observed_json, reconciled_at
FROM task_projections_v1;

DROP TABLE task_projections_v1;
CREATE INDEX task_projections_effect ON task_projections(effect_id, projection_id);
"#;

const MIGRATION_3: &str = r#"
ALTER TABLE outbox ADD COLUMN superseded_at INTEGER;
ALTER TABLE outbox ADD COLUMN superseded_by_event_id TEXT;
CREATE INDEX outbox_dispatchable
    ON outbox(delivered_at, superseded_at, lease_until, created_at);
"#;

#[derive(Clone, Debug, Serialize)]
pub struct EventInput {
    pub event_id: String,
    pub event_type: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct EffectInput {
    pub effect_id: String,
    pub effect_type: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunInput {
    pub run_id: String,
    pub task_id: String,
    pub role: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoredRun {
    pub run_id: String,
    pub case_key: String,
    pub event_id: String,
    pub task_id: String,
    pub role: String,
    pub payload: Value,
    pub accepted_at: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceInput {
    pub evidence_id: String,
    pub kind: String,
    pub source: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct FindingInput {
    pub finding_id: String,
    pub origin_role: String,
    pub reviewed_head_sha: String,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct PolicyInput {
    pub repository_id: u64,
    pub revision: u64,
    pub accepted_at: u64,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaskProjectionInput {
    pub projection_id: String,
    pub effect_id: String,
    pub board: String,
    pub task_id: String,
    pub desired: Value,
    pub observed: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct NewCase {
    pub case_key: String,
    pub repository_id: u64,
    pub issue_number: u64,
    pub workflow_version: u32,
    pub policy_revision: u64,
    pub initial_state: String,
    pub observed_at: u64,
    pub event: EventInput,
    pub effects: Vec<EffectInput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TransitionInput {
    pub case_key: String,
    pub expected_revision: u64,
    pub next_state: String,
    pub remediation_round: u32,
    pub plan_version: u32,
    pub pr_number: Option<u64>,
    pub head_sha: Option<String>,
    pub observed_at: u64,
    pub event: EventInput,
    pub run: Option<RunInput>,
    pub evidence: Vec<EvidenceInput>,
    pub findings: Vec<FindingInput>,
    pub effects: Vec<EffectInput>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultPoint {
    AfterEvent,
    AfterRun,
    AfterEvidence,
    AfterProjection,
    AfterOutbox,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyResult {
    Applied,
    Replayed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StoredCase {
    pub case_key: String,
    pub repository_id: u64,
    pub issue_number: u64,
    pub workflow_version: u32,
    pub state: String,
    pub state_revision: u64,
    pub policy_revision: u64,
    pub remediation_round: u32,
    pub plan_version: u32,
    pub pr_number: Option<u64>,
    pub head_sha: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LedgerStatus {
    pub schema_version: u32,
    pub cases: Vec<StoredCase>,
    pub events: u64,
    pub runs: u64,
    pub evidence: u64,
    pub findings: u64,
    pub outbox_total: u64,
    pub outbox_pending: u64,
    pub outbox_leased: u64,
    pub outbox_delivered: u64,
    pub outbox_superseded: u64,
    pub task_projections: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedEffect {
    pub effect_id: String,
    pub case_key: String,
    pub state_revision: u64,
    pub effect_type: String,
    pub payload: Value,
    pub lease_owner: String,
    pub lease_until: u64,
}

#[derive(Debug)]
pub enum StoreError {
    Database(rusqlite::Error),
    Serialization(serde_json::Error),
    InvalidInteger,
    InvalidInput(&'static str),
    MissingCase(String),
    StaleRevision { expected: u64, actual: u64 },
    IdempotencyConflict { id: String },
    InjectedFault(FaultPoint),
    LeaseLost(String),
    ReadOnly,
    DestinationExists(PathBuf),
    UnsupportedSchema(u32),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::Serialization(error) => error.fmt(formatter),
            Self::InvalidInteger => formatter.write_str("integer cannot be represented by SQLite"),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::MissingCase(case) => write!(formatter, "case not found: {case}"),
            Self::StaleRevision { expected, actual } => {
                write!(
                    formatter,
                    "stale revision: expected {expected}, actual {actual}"
                )
            }
            Self::IdempotencyConflict { id } => write!(formatter, "idempotency conflict: {id}"),
            Self::InjectedFault(point) => write!(formatter, "injected fault: {point:?}"),
            Self::LeaseLost(effect) => write!(formatter, "outbox lease lost: {effect}"),
            Self::ReadOnly => formatter.write_str("ledger handle is read-only"),
            Self::DestinationExists(path) => {
                write!(
                    formatter,
                    "restore destination already exists: {}",
                    path.display()
                )
            }
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported schema version: {version}")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    connection: Connection,
    path: PathBuf,
    read_only: bool,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut connection = Connection::open(&path)?;
        configure(&connection)?;
        migrate(&mut connection)?;
        Ok(Self {
            connection,
            path,
            read_only: false,
        })
    }

    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let version = schema_version(&connection)?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(version));
        }
        Ok(Self {
            connection,
            path,
            read_only: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            Err(StoreError::ReadOnly)
        } else {
            Ok(())
        }
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema_version(&self.connection)
    }

    pub fn foreign_keys_enabled(&self) -> Result<bool> {
        let enabled: i64 = self
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        Ok(enabled == 1)
    }

    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .connection
            .pragma_query_value(None, "journal_mode", |row| row.get(0))?)
    }

    pub fn record_policy(&mut self, input: &PolicyInput) -> Result<ApplyResult> {
        self.ensure_writable()?;
        if input.repository_id == 0 || input.revision == 0 || !input.payload.is_object() {
            return Err(StoreError::InvalidInput(
                "policy repository, revision, and object payload are required",
            ));
        }
        let (payload_json, payload_hash) = payload(&input.payload)?;
        let existing: Option<String> = self
            .connection
            .query_row(
                "SELECT payload_sha256 FROM policies WHERE repository_id = ?1 AND revision = ?2",
                params![sql_u64(input.repository_id)?, sql_u64(input.revision)?],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing_hash) = existing {
            if existing_hash == payload_hash {
                return Ok(ApplyResult::Replayed);
            }
            return Err(StoreError::IdempotencyConflict {
                id: format!("policy:{}:{}", input.repository_id, input.revision),
            });
        }
        self.connection.execute(
            "INSERT INTO policies(repository_id, revision, payload_json, payload_sha256, accepted_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                sql_u64(input.repository_id)?,
                sql_u64(input.revision)?,
                payload_json,
                payload_hash,
                sql_u64(input.accepted_at)?,
            ],
        )?;
        Ok(ApplyResult::Applied)
    }

    pub fn create_case(&mut self, input: &NewCase) -> Result<ApplyResult> {
        self.ensure_writable()?;
        validate_common(&input.case_key, &input.event)?;
        let command_hash = hash_serialized(input)?;
        if let Some(existing) = existing_event(&self.connection, &input.event.event_id)? {
            return replay_or_conflict(
                existing,
                &input.case_key,
                &command_hash,
                &input.event.event_id,
            );
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO cases(case_key, repository_id, issue_number, workflow_version, state, state_revision, policy_revision, remediation_round, plan_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 0, 0, ?7, ?7)",
            params![
                input.case_key,
                sql_u64(input.repository_id)?,
                sql_u64(input.issue_number)?,
                i64::from(input.workflow_version),
                input.initial_state,
                sql_u64(input.policy_revision)?,
                sql_u64(input.observed_at)?,
            ],
        )?;
        insert_event(
            &transaction,
            EventRecord {
                case_key: &input.case_key,
                revision: 1,
                observed_at: input.observed_at,
                event: &input.event,
                command_hash: &command_hash,
                previous_state: None,
                next_state: &input.initial_state,
                policy_revision: input.policy_revision,
                remediation_round: 0,
                plan_version: 0,
                pr_number: None,
                head_sha: None,
            },
        )?;
        insert_effects(
            &transaction,
            &input.case_key,
            1,
            input.observed_at,
            &input.effects,
        )?;
        transaction.commit()?;
        Ok(ApplyResult::Applied)
    }

    pub fn apply_transition(
        &mut self,
        input: &TransitionInput,
        fault: Option<FaultPoint>,
    ) -> Result<ApplyResult> {
        self.ensure_writable()?;
        validate_common(&input.case_key, &input.event)?;
        let command_hash = hash_serialized(input)?;
        if let Some(existing) = existing_event(&self.connection, &input.event.event_id)? {
            return replay_or_conflict(
                existing,
                &input.case_key,
                &command_hash,
                &input.event.event_id,
            );
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = query_case(&transaction, &input.case_key)?
            .ok_or_else(|| StoreError::MissingCase(input.case_key.clone()))?;
        if current.state_revision != input.expected_revision {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                actual: current.state_revision,
            });
        }
        let next_revision = input
            .expected_revision
            .checked_add(1)
            .ok_or(StoreError::InvalidInteger)?;
        insert_event(
            &transaction,
            EventRecord {
                case_key: &input.case_key,
                revision: next_revision,
                observed_at: input.observed_at,
                event: &input.event,
                command_hash: &command_hash,
                previous_state: Some(&current.state),
                next_state: &input.next_state,
                policy_revision: current.policy_revision,
                remediation_round: input.remediation_round,
                plan_version: input.plan_version,
                pr_number: input.pr_number,
                head_sha: input.head_sha.as_deref(),
            },
        )?;
        inject(fault, FaultPoint::AfterEvent)?;
        if let Some(run) = &input.run {
            insert_run(
                &transaction,
                &input.case_key,
                &input.event.event_id,
                input.observed_at,
                run,
            )?;
        }
        inject(fault, FaultPoint::AfterRun)?;
        insert_evidence(
            &transaction,
            &input.case_key,
            input.observed_at,
            &input.evidence,
        )?;
        insert_findings(
            &transaction,
            &input.case_key,
            input.observed_at,
            &input.findings,
        )?;
        inject(fault, FaultPoint::AfterEvidence)?;
        let updated = transaction.execute(
            "UPDATE cases SET state = ?1, state_revision = ?2, remediation_round = ?3, plan_version = ?4, pr_number = ?5, head_sha = ?6, updated_at = ?7
             WHERE case_key = ?8 AND state_revision = ?9",
            params![
                input.next_state,
                sql_u64(next_revision)?,
                i64::from(input.remediation_round),
                i64::from(input.plan_version),
                input.pr_number.map(sql_u64).transpose()?,
                input.head_sha.as_deref(),
                sql_u64(input.observed_at)?,
                input.case_key,
                sql_u64(input.expected_revision)?,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::StaleRevision {
                expected: input.expected_revision,
                actual: current.state_revision,
            });
        }
        inject(fault, FaultPoint::AfterProjection)?;
        transaction.execute(
            "UPDATE outbox
             SET superseded_at = ?1, superseded_by_event_id = ?2,
                 lease_owner = NULL, lease_until = NULL
             WHERE case_key = ?3 AND state_revision < ?4
               AND delivered_at IS NULL AND superseded_at IS NULL",
            params![
                sql_u64(input.observed_at)?,
                input.event.event_id,
                input.case_key,
                sql_u64(next_revision)?,
            ],
        )?;
        insert_effects(
            &transaction,
            &input.case_key,
            next_revision,
            input.observed_at,
            &input.effects,
        )?;
        inject(fault, FaultPoint::AfterOutbox)?;
        transaction.commit()?;
        Ok(ApplyResult::Applied)
    }

    pub fn case(&self, case_key: &str) -> Result<Option<StoredCase>> {
        query_case(&self.connection, case_key)
    }

    pub fn reconstruct_case(&self, case_key: &str) -> Result<Option<StoredCase>> {
        Ok(self
            .connection
            .query_row(
                "SELECT e.next_state, e.state_revision, e.policy_revision, e.remediation_round,
                        e.plan_version, e.pr_number, e.head_sha,
                        c.repository_id, c.issue_number, c.workflow_version
                 FROM events e JOIN cases c ON c.case_key = e.case_key
                 WHERE e.case_key = ?1 ORDER BY e.state_revision DESC LIMIT 1",
                [case_key],
                |row| {
                    let revision: i64 = row.get(1)?;
                    let policy: i64 = row.get(2)?;
                    let remediation: i64 = row.get(3)?;
                    let plan: i64 = row.get(4)?;
                    let pr_number: Option<i64> = row.get(5)?;
                    Ok(StoredCase {
                        case_key: case_key.to_owned(),
                        repository_id: unsigned(row.get(7)?),
                        issue_number: unsigned(row.get(8)?),
                        workflow_version: u32::try_from(row.get::<_, i64>(9)?).unwrap_or_default(),
                        state: row.get(0)?,
                        state_revision: unsigned(revision),
                        policy_revision: unsigned(policy),
                        remediation_round: u32::try_from(remediation).unwrap_or_default(),
                        plan_version: u32::try_from(plan).unwrap_or_default(),
                        pr_number: pr_number.map(unsigned),
                        head_sha: row.get(6)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn projection_matches_history(&self, case_key: &str) -> Result<bool> {
        Ok(self.case(case_key)? == self.reconstruct_case(case_key)?)
    }

    pub fn repair_case_projection(&mut self, case_key: &str) -> Result<()> {
        self.ensure_writable()?;
        let rebuilt = self
            .reconstruct_case(case_key)?
            .ok_or_else(|| StoreError::MissingCase(case_key.to_owned()))?;
        let updated = self.connection.execute(
            "UPDATE cases SET state = ?1, state_revision = ?2, policy_revision = ?3, remediation_round = ?4, plan_version = ?5, pr_number = ?6, head_sha = ?7
             WHERE case_key = ?8",
            params![
                rebuilt.state,
                sql_u64(rebuilt.state_revision)?,
                sql_u64(rebuilt.policy_revision)?,
                i64::from(rebuilt.remediation_round),
                i64::from(rebuilt.plan_version),
                rebuilt.pr_number.map(sql_u64).transpose()?,
                rebuilt.head_sha,
                case_key,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::MissingCase(case_key.to_owned()));
        }
        Ok(())
    }

    pub fn event_count(&self) -> Result<u64> {
        count(&self.connection, "events")
    }

    pub fn run_count(&self) -> Result<u64> {
        count(&self.connection, "runs")
    }

    pub fn run_by_task_id(&self, task_id: &str) -> Result<Option<StoredRun>> {
        self.connection
            .query_row(
                "SELECT run_id, case_key, event_id, task_id, role, payload_json, accepted_at
                 FROM runs WHERE task_id = ?1",
                [task_id],
                stored_run,
            )
            .optional()?
            .map(parse_stored_run)
            .transpose()
    }

    pub fn runs_for_case(&self, case_key: &str) -> Result<Vec<StoredRun>> {
        let mut statement = self.connection.prepare(
            "SELECT run_id, case_key, event_id, task_id, role, payload_json, accepted_at
             FROM runs WHERE case_key = ?1 ORDER BY accepted_at, run_id",
        )?;
        statement
            .query_map([case_key], stored_run)?
            .map(|row| row.map_err(StoreError::from).and_then(parse_stored_run))
            .collect()
    }

    pub fn outbox_count(&self) -> Result<u64> {
        count(&self.connection, "outbox")
    }

    pub fn evidence_count(&self) -> Result<u64> {
        count(&self.connection, "evidence")
    }

    pub fn finding_count(&self) -> Result<u64> {
        count(&self.connection, "findings")
    }

    pub fn task_projection_count(&self) -> Result<u64> {
        count(&self.connection, "task_projections")
    }

    pub fn status(&self, now: u64) -> Result<LedgerStatus> {
        let mut statement = self.connection.prepare(
            "SELECT case_key, repository_id, issue_number, workflow_version, state,
                    state_revision, policy_revision, remediation_round,
                    plan_version, pr_number, head_sha
             FROM cases ORDER BY repository_id, issue_number, workflow_version",
        )?;
        let cases = statement
            .query_map([], |row| {
                let state_revision: i64 = row.get(5)?;
                let policy_revision: i64 = row.get(6)?;
                let remediation_round: i64 = row.get(7)?;
                let plan_version: i64 = row.get(8)?;
                let pr_number: Option<i64> = row.get(9)?;
                Ok(StoredCase {
                    case_key: row.get(0)?,
                    repository_id: unsigned(row.get(1)?),
                    issue_number: unsigned(row.get(2)?),
                    workflow_version: u32::try_from(row.get::<_, i64>(3)?).unwrap_or_default(),
                    state: row.get(4)?,
                    state_revision: unsigned(state_revision),
                    policy_revision: unsigned(policy_revision),
                    remediation_round: u32::try_from(remediation_round).unwrap_or_default(),
                    plan_version: u32::try_from(plan_version).unwrap_or_default(),
                    pr_number: pr_number.map(unsigned),
                    head_sha: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let now = sql_u64(now)?;
        let (pending, leased, delivered, superseded): (i64, i64, i64, i64) =
            self.connection.query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN delivered_at IS NULL AND superseded_at IS NULL THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN delivered_at IS NULL AND superseded_at IS NULL AND lease_until >= ?1 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN delivered_at IS NOT NULL THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN superseded_at IS NOT NULL THEN 1 ELSE 0 END), 0)
             FROM outbox",
            [now],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        Ok(LedgerStatus {
            schema_version: self.schema_version()?,
            cases,
            events: self.event_count()?,
            runs: self.run_count()?,
            evidence: self.evidence_count()?,
            findings: self.finding_count()?,
            outbox_total: self.outbox_count()?,
            outbox_pending: unsigned(pending),
            outbox_leased: unsigned(leased),
            outbox_delivered: unsigned(delivered),
            outbox_superseded: unsigned(superseded),
            task_projections: self.task_projection_count()?,
        })
    }

    pub fn task_projection(&self, projection_id: &str) -> Result<Option<TaskProjectionInput>> {
        self.connection
            .query_row(
                "SELECT projection_id, effect_id, board, task_id, desired_json, observed_json
                 FROM task_projections WHERE projection_id = ?1",
                [projection_id],
                |row| {
                    let desired: String = row.get(4)?;
                    let observed: String = row.get(5)?;
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        desired,
                        observed,
                    ))
                },
            )
            .optional()?
            .map(
                |(projection_id, effect_id, board, task_id, desired, observed)| -> Result<TaskProjectionInput> {
                    Ok(TaskProjectionInput {
                        projection_id,
                        effect_id,
                        board,
                        task_id,
                        desired: serde_json::from_str(&desired)?,
                        observed: serde_json::from_str(&observed)?,
                    })
                },
            )
            .transpose()
    }

    pub fn unconsumed_task_projections(&self) -> Result<Vec<TaskProjectionInput>> {
        let mut statement = self.connection.prepare(
            "SELECT p.projection_id, p.effect_id, p.board, p.task_id,
                    p.desired_json, p.observed_json
             FROM task_projections p
             LEFT JOIN runs r ON r.task_id = p.task_id
             WHERE p.task_id IS NOT NULL AND r.task_id IS NULL
             ORDER BY p.reconciled_at, p.projection_id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .map(|row| {
                let (projection_id, effect_id, board, task_id, desired, observed) = row?;
                Ok(TaskProjectionInput {
                    projection_id,
                    effect_id,
                    board,
                    task_id,
                    desired: serde_json::from_str(&desired)?,
                    observed: serde_json::from_str(&observed)?,
                })
            })
            .collect()
    }

    pub fn claim_effect(
        &mut self,
        owner: &str,
        now: u64,
        lease_seconds: u64,
    ) -> Result<Option<ClaimedEffect>> {
        self.claim_effect_inner(owner, now, lease_seconds, None)
    }

    pub fn claim_effect_matching(
        &mut self,
        owner: &str,
        now: u64,
        lease_seconds: u64,
        effect_types: &[&str],
    ) -> Result<Option<ClaimedEffect>> {
        let unique = effect_types
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if effect_types.is_empty()
            || unique.len() != effect_types.len()
            || effect_types.iter().any(|value| value.trim().is_empty())
        {
            return Err(StoreError::InvalidInput(
                "unique nonempty effect types are required",
            ));
        }
        self.claim_effect_inner(owner, now, lease_seconds, Some(effect_types))
    }

    fn claim_effect_inner(
        &mut self,
        owner: &str,
        now: u64,
        lease_seconds: u64,
        effect_types: Option<&[&str]>,
    ) -> Result<Option<ClaimedEffect>> {
        self.ensure_writable()?;
        if owner.trim().is_empty() || lease_seconds == 0 {
            return Err(StoreError::InvalidInput(
                "owner and positive lease are required",
            ));
        }
        let lease_until = now
            .checked_add(lease_seconds)
            .ok_or(StoreError::InvalidInteger)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidate: Option<(String, String, i64, String, String)> = {
            let mut statement = transaction.prepare(
                "SELECT effect_id, case_key, state_revision, effect_type, payload_json
                 FROM outbox
                 WHERE delivered_at IS NULL AND superseded_at IS NULL
                   AND (lease_until IS NULL OR lease_until < ?1)
                 ORDER BY created_at, effect_id",
            )?;
            let mut rows = statement.query([sql_u64(now)?])?;
            let mut found = None;
            while let Some(row) = rows.next()? {
                let effect_type: String = row.get(3)?;
                if effect_types.is_none_or(|allowed| allowed.contains(&effect_type.as_str())) {
                    found = Some((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        effect_type,
                        row.get(4)?,
                    ));
                    break;
                }
            }
            found
        };
        let Some((effect_id, case_key, state_revision, effect_type, payload_json)) = candidate
        else {
            transaction.commit()?;
            return Ok(None);
        };
        let updated = transaction.execute(
            "UPDATE outbox SET lease_owner = ?1, lease_until = ?2
             WHERE effect_id = ?3 AND delivered_at IS NULL AND superseded_at IS NULL
               AND (lease_until IS NULL OR lease_until < ?4)",
            params![owner, sql_u64(lease_until)?, effect_id, sql_u64(now)?],
        )?;
        if updated != 1 {
            transaction.commit()?;
            return Ok(None);
        }
        transaction.commit()?;
        Ok(Some(ClaimedEffect {
            effect_id,
            case_key,
            state_revision: unsigned(state_revision),
            effect_type,
            payload: serde_json::from_str(&payload_json)?,
            lease_owner: owner.to_owned(),
            lease_until,
        }))
    }

    pub fn acknowledge_effect(&mut self, effect_id: &str, owner: &str, now: u64) -> Result<()> {
        self.ensure_writable()?;
        let updated = self.connection.execute(
            "UPDATE outbox SET delivered_at = ?1, lease_owner = NULL, lease_until = NULL
             WHERE effect_id = ?2 AND lease_owner = ?3 AND lease_until >= ?1
               AND delivered_at IS NULL AND superseded_at IS NULL",
            params![sql_u64(now)?, effect_id, owner],
        )?;
        if updated != 1 {
            return Err(StoreError::LeaseLost(effect_id.to_owned()));
        }
        Ok(())
    }

    pub fn complete_task_projection(
        &mut self,
        input: &TaskProjectionInput,
        owner: &str,
        now: u64,
        fault: Option<FaultPoint>,
    ) -> Result<ApplyResult> {
        self.complete_task_projections(std::slice::from_ref(input), owner, now, fault)
    }

    pub fn complete_task_projections(
        &mut self,
        inputs: &[TaskProjectionInput],
        owner: &str,
        now: u64,
        fault: Option<FaultPoint>,
    ) -> Result<ApplyResult> {
        self.ensure_writable()?;
        let effect_id = inputs.first().map(|input| input.effect_id.as_str());
        let unique = inputs
            .iter()
            .map(|input| input.projection_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if inputs.is_empty()
            || unique.len() != inputs.len()
            || owner.trim().is_empty()
            || effect_id.is_none()
            || inputs.iter().any(|input| {
                input.projection_id.trim().is_empty()
                    || input.effect_id.trim().is_empty()
                    || input.board.trim().is_empty()
                    || input.task_id.trim().is_empty()
                    || !input.desired.is_object()
                    || !input.observed.is_object()
                    || Some(input.effect_id.as_str()) != effect_id
            })
        {
            return Err(StoreError::InvalidInput(
                "unique projections for one effect, board, task, object evidence, and owner are required",
            ));
        }
        let effect_id = effect_id.expect("validated effect ID");
        let serialized = inputs
            .iter()
            .map(|input| {
                Ok((
                    input,
                    serde_json::to_string(&input.desired)?,
                    serde_json::to_string(&input.observed)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (case_key, delivered, superseded): (String, Option<i64>, Option<i64>) = transaction
            .query_row(
                "SELECT case_key, delivered_at, superseded_at FROM outbox WHERE effect_id = ?1",
                [effect_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| StoreError::LeaseLost(effect_id.to_owned()))?;
        let existing_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM task_projections WHERE effect_id = ?1",
            [effect_id],
            |row| row.get(0),
        )?;
        if delivered.is_some() {
            if usize::try_from(existing_count).ok() != Some(inputs.len()) {
                return Err(StoreError::IdempotencyConflict {
                    id: effect_id.to_owned(),
                });
            }
            for (input, desired_json, observed_json) in &serialized {
                let existing: Option<(String, String, String, String, String)> = transaction
                    .query_row(
                        "SELECT effect_id, board, task_id, desired_json, observed_json
                         FROM task_projections WHERE projection_id = ?1",
                        [&input.projection_id],
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
                if existing.as_ref()
                    != Some(&(
                        input.effect_id.clone(),
                        input.board.clone(),
                        input.task_id.clone(),
                        desired_json.clone(),
                        observed_json.clone(),
                    ))
                {
                    return Err(StoreError::IdempotencyConflict {
                        id: input.projection_id.clone(),
                    });
                }
            }
            transaction.commit()?;
            return Ok(ApplyResult::Replayed);
        }
        if superseded.is_some() {
            return Err(StoreError::LeaseLost(effect_id.to_owned()));
        }
        let lease_valid: bool = transaction.query_row(
            "SELECT EXISTS(
                    SELECT 1 FROM outbox
                    WHERE effect_id = ?1 AND delivered_at IS NULL AND superseded_at IS NULL
                      AND lease_owner = ?2 AND lease_until >= ?3
                )",
            params![effect_id, owner, sql_u64(now)?],
            |row| row.get(0),
        )?;
        if !lease_valid || existing_count != 0 {
            return Err(StoreError::LeaseLost(effect_id.to_owned()));
        }
        for (input, desired_json, observed_json) in &serialized {
            let existing_projection: Option<String> = transaction
                .query_row(
                    "SELECT effect_id FROM task_projections WHERE projection_id = ?1",
                    [&input.projection_id],
                    |row| row.get(0),
                )
                .optional()?;
            if existing_projection.is_some() {
                return Err(StoreError::IdempotencyConflict {
                    id: input.projection_id.clone(),
                });
            }
            transaction.execute(
                "INSERT INTO task_projections(projection_id, case_key, effect_id, board, task_id, desired_json, observed_json, reconciled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    input.projection_id,
                    case_key,
                    input.effect_id,
                    input.board,
                    input.task_id,
                    desired_json,
                    observed_json,
                    sql_u64(now)?,
                ],
            )?;
        }
        inject(fault, FaultPoint::AfterProjection)?;
        let updated = transaction.execute(
            "UPDATE outbox SET delivered_at = ?1, lease_owner = NULL, lease_until = NULL
             WHERE effect_id = ?2 AND lease_owner = ?3 AND lease_until >= ?1
               AND delivered_at IS NULL AND superseded_at IS NULL",
            params![sql_u64(now)?, effect_id, owner],
        )?;
        if updated != 1 {
            return Err(StoreError::LeaseLost(effect_id.to_owned()));
        }
        transaction.commit()?;
        Ok(ApplyResult::Applied)
    }

    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<()> {
        self.connection.backup(MAIN_DB, destination, None)?;
        Ok(())
    }

    pub fn restore_backup_to_new(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<()> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(StoreError::DestinationExists(destination.to_path_buf()));
        }
        let source = Self::open_read_only(source)?;
        source.backup_to(destination)?;
        let restored = Self::open_read_only(destination)?;
        if restored.schema_version()? != SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema(restored.schema_version()?));
        }
        Ok(())
    }
}

fn configure(connection: &Connection) -> Result<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let mut version = schema_version(connection)?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema(version));
    }
    if version == 0 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        transaction.execute_batch(MIGRATION_1)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 0)",
            [],
        )?;
        transaction.pragma_update(None, "user_version", 1)?;
        transaction.commit()?;
        version = 1;
    }
    if version == 1 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        transaction.execute_batch(MIGRATION_2)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, 0)",
            [],
        )?;
        transaction.pragma_update(None, "user_version", 2)?;
        transaction.commit()?;
        version = 2;
    }
    if version == 2 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        transaction.execute_batch(MIGRATION_3)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (3, 0)",
            [],
        )?;
        transaction.pragma_update(None, "user_version", 3)?;
        transaction.commit()?;
        version = 3;
    }
    if version != SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema(version));
    }
    Ok(())
}

fn schema_version(connection: &Connection) -> Result<u32> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    u32::try_from(version).map_err(|_| StoreError::InvalidInteger)
}

fn validate_common(case_key: &str, event: &EventInput) -> Result<()> {
    if case_key.trim().is_empty()
        || event.event_id.trim().is_empty()
        || event.event_type.trim().is_empty()
        || !event.payload.is_object()
    {
        return Err(StoreError::InvalidInput(
            "case, event identity, type, and object payload are required",
        ));
    }
    Ok(())
}

fn sql_u64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| StoreError::InvalidInteger)
}

fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

fn hash_serialized(value: &impl Serialize) -> Result<String> {
    Ok(hash_bytes(&serde_json::to_vec(value)?))
}

fn payload(value: &Value) -> Result<(String, String)> {
    let serialized = serde_json::to_string(value)?;
    let digest = hash_bytes(serialized.as_bytes());
    Ok((serialized, digest))
}

struct EventRecord<'a> {
    case_key: &'a str,
    revision: u64,
    observed_at: u64,
    event: &'a EventInput,
    command_hash: &'a str,
    previous_state: Option<&'a str>,
    next_state: &'a str,
    policy_revision: u64,
    remediation_round: u32,
    plan_version: u32,
    pr_number: Option<u64>,
    head_sha: Option<&'a str>,
}

fn insert_event(transaction: &Transaction<'_>, record: EventRecord<'_>) -> Result<()> {
    let (payload_json, payload_hash) = payload(&record.event.payload)?;
    transaction.execute(
        "INSERT INTO events(event_id, case_key, state_revision, observed_at, event_type, payload_json, payload_sha256, command_sha256, previous_state, next_state, policy_revision, remediation_round, plan_version, pr_number, head_sha)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            record.event.event_id,
            record.case_key,
            sql_u64(record.revision)?,
            sql_u64(record.observed_at)?,
            record.event.event_type,
            payload_json,
            payload_hash,
            record.command_hash,
            record.previous_state,
            record.next_state,
            sql_u64(record.policy_revision)?,
            i64::from(record.remediation_round),
            i64::from(record.plan_version),
            record.pr_number.map(sql_u64).transpose()?,
            record.head_sha,
        ],
    )?;
    Ok(())
}

fn insert_run(
    transaction: &Transaction<'_>,
    case_key: &str,
    event_id: &str,
    accepted_at: u64,
    run: &RunInput,
) -> Result<()> {
    if run.run_id.trim().is_empty() || run.task_id.trim().is_empty() || run.role.trim().is_empty() {
        return Err(StoreError::InvalidInput(
            "run identity and role are required",
        ));
    }
    let (payload_json, payload_hash) = payload(&run.payload)?;
    transaction.execute(
        "INSERT INTO runs(run_id, case_key, event_id, task_id, role, payload_json, payload_sha256, accepted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            run.run_id,
            case_key,
            event_id,
            run.task_id,
            run.role,
            payload_json,
            payload_hash,
            sql_u64(accepted_at)?,
        ],
    )?;
    Ok(())
}

fn insert_evidence(
    transaction: &Transaction<'_>,
    case_key: &str,
    observed_at: u64,
    evidence: &[EvidenceInput],
) -> Result<()> {
    for item in evidence {
        if item.evidence_id.trim().is_empty()
            || item.kind.trim().is_empty()
            || item.source.trim().is_empty()
            || !item.payload.is_object()
        {
            return Err(StoreError::InvalidInput(
                "evidence identity, kind, source, and object payload are required",
            ));
        }
        let (payload_json, payload_hash) = payload(&item.payload)?;
        transaction.execute(
            "INSERT INTO evidence(evidence_id, case_key, kind, source, observed_at, payload_json, payload_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item.evidence_id,
                case_key,
                item.kind,
                item.source,
                sql_u64(observed_at)?,
                payload_json,
                payload_hash,
            ],
        )?;
    }
    Ok(())
}

fn insert_findings(
    transaction: &Transaction<'_>,
    case_key: &str,
    recorded_at: u64,
    findings: &[FindingInput],
) -> Result<()> {
    for item in findings {
        if item.finding_id.trim().is_empty()
            || item.origin_role.trim().is_empty()
            || item.reviewed_head_sha.len() != 40
            || !item.payload.is_object()
        {
            return Err(StoreError::InvalidInput(
                "finding identity, origin, exact head, and object payload are required",
            ));
        }
        let (payload_json, payload_hash) = payload(&item.payload)?;
        transaction.execute(
            "INSERT INTO findings(finding_id, case_key, origin_role, reviewed_head_sha, payload_json, payload_sha256, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item.finding_id,
                case_key,
                item.origin_role,
                item.reviewed_head_sha,
                payload_json,
                payload_hash,
                sql_u64(recorded_at)?,
            ],
        )?;
    }
    Ok(())
}

fn insert_effects(
    transaction: &Transaction<'_>,
    case_key: &str,
    state_revision: u64,
    created_at: u64,
    effects: &[EffectInput],
) -> Result<()> {
    for effect in effects {
        if effect.effect_id.trim().is_empty() || effect.effect_type.trim().is_empty() {
            return Err(StoreError::InvalidInput(
                "effect identity and type are required",
            ));
        }
        let (payload_json, payload_hash) = payload(&effect.payload)?;
        transaction.execute(
            "INSERT INTO outbox(effect_id, case_key, state_revision, effect_type, payload_json, payload_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                effect.effect_id,
                case_key,
                sql_u64(state_revision)?,
                effect.effect_type,
                payload_json,
                payload_hash,
                sql_u64(created_at)?,
            ],
        )?;
    }
    Ok(())
}

fn inject(requested: Option<FaultPoint>, current: FaultPoint) -> Result<()> {
    if requested == Some(current) {
        return Err(StoreError::InjectedFault(current));
    }
    Ok(())
}

fn existing_event(connection: &Connection, event_id: &str) -> Result<Option<(String, String)>> {
    Ok(connection
        .query_row(
            "SELECT case_key, command_sha256 FROM events WHERE event_id = ?1",
            [event_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn replay_or_conflict(
    existing: (String, String),
    case_key: &str,
    command_hash: &str,
    event_id: &str,
) -> Result<ApplyResult> {
    if existing.0 == case_key && existing.1 == command_hash {
        Ok(ApplyResult::Replayed)
    } else {
        Err(StoreError::IdempotencyConflict {
            id: event_id.to_owned(),
        })
    }
}

fn query_case(connection: &Connection, case_key: &str) -> Result<Option<StoredCase>> {
    Ok(connection
        .query_row(
            "SELECT case_key, repository_id, issue_number, workflow_version, state,
                    state_revision, policy_revision, remediation_round, plan_version,
                    pr_number, head_sha FROM cases WHERE case_key = ?1",
            [case_key],
            |row| {
                let revision: i64 = row.get(5)?;
                let policy: i64 = row.get(6)?;
                let remediation: i64 = row.get(7)?;
                let plan: i64 = row.get(8)?;
                let pr_number: Option<i64> = row.get(9)?;
                Ok(StoredCase {
                    case_key: row.get(0)?,
                    repository_id: unsigned(row.get(1)?),
                    issue_number: unsigned(row.get(2)?),
                    workflow_version: u32::try_from(row.get::<_, i64>(3)?).unwrap_or_default(),
                    state: row.get(4)?,
                    state_revision: unsigned(revision),
                    policy_revision: unsigned(policy),
                    remediation_round: u32::try_from(remediation).unwrap_or_default(),
                    plan_version: u32::try_from(plan).unwrap_or_default(),
                    pr_number: pr_number.map(unsigned),
                    head_sha: row.get(10)?,
                })
            },
        )
        .optional()?)
}

type StoredRunRow = (String, String, String, String, String, String, i64);

fn stored_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRunRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn parse_stored_run(row: StoredRunRow) -> Result<StoredRun> {
    Ok(StoredRun {
        run_id: row.0,
        case_key: row.1,
        event_id: row.2,
        task_id: row.3,
        role: row.4,
        payload: serde_json::from_str(&row.5)?,
        accepted_at: unsigned(row.6),
    })
}

fn count(connection: &Connection, table: &str) -> Result<u64> {
    let sql = match table {
        "events" => "SELECT COUNT(*) FROM events",
        "runs" => "SELECT COUNT(*) FROM runs",
        "outbox" => "SELECT COUNT(*) FROM outbox",
        "evidence" => "SELECT COUNT(*) FROM evidence",
        "findings" => "SELECT COUNT(*) FROM findings",
        "task_projections" => "SELECT COUNT(*) FROM task_projections",
        _ => return Err(StoreError::InvalidInput("unknown count table")),
    };
    let value: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    Ok(unsigned(value))
}
