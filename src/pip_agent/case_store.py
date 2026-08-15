from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import threading
from collections.abc import Mapping
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .state_machine import CaseState, TransitionError, transition


class CaseStoreError(RuntimeError):
    """Permanent case storage failed."""


class ImmutableRunError(CaseStoreError):
    """An immutable task ID was reused."""


def _now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def _fresh_now(previous: str) -> str:
    current = _now()
    while current == previous:
        current = _now()
    return current


def _canonical(payload: Mapping[str, Any]) -> str:
    return json.dumps(dict(payload), sort_keys=True, separators=(",", ":"))


CASE_STATES = frozenset(state.value for state in CaseState)
MAX_REMEDIATION_ROUNDS = 3


class CaseStore:
    """Control-plane store; writable database access is a trusted boundary.

    Triggers and connection capabilities prevent accidental bypass through normal
    callers. They are not cryptographic protection from the database file owner,
    who can replace the file or drop its schema.
    """

    def __init__(self, path: Path) -> None:
        self.path = path
        self._write_guard = threading.local()
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        if path.is_symlink():
            raise CaseStoreError("case database path must not be a symlink")
        if not path.exists():
            descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
            os.close(descriptor)
        if path.stat().st_uid != os.geteuid():
            raise CaseStoreError(
                "case database must be owned by the control-plane user"
            )
        path.chmod(0o600)
        self._initialize()

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.path)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA foreign_keys = ON")
        connection.execute("PRAGMA journal_mode = WAL")
        for suffix in ("-wal", "-shm"):
            auxiliary = Path(f"{self.path}{suffix}")
            if auxiliary.exists():
                auxiliary.chmod(0o600)
        connection.create_function(
            "pip_case_write_authorized",
            0,
            lambda: int(bool(getattr(self._write_guard, "enabled", False))),
        )
        return connection

    @contextmanager
    def _authorized_case_write(self) -> Any:
        if getattr(self._write_guard, "enabled", False):
            raise CaseStoreError("nested case write authorization is forbidden")
        self._write_guard.enabled = True
        try:
            yield
        finally:
            self._write_guard.enabled = False

    def _initialize(self) -> None:
        with self._connect() as db:
            db.executescript(
                """
                CREATE TABLE IF NOT EXISTS cases (
                    case_id TEXT PRIMARY KEY,
                    repository TEXT NOT NULL,
                    issue_number INTEGER NOT NULL CHECK(issue_number > 0),
                    workflow_version INTEGER NOT NULL DEFAULT 2
                        CHECK(workflow_version = 2),
                    intake_label TEXT NOT NULL,
                    state TEXT NOT NULL CHECK(state IN (
                        'PLANNING', 'WAITING_HUMAN', 'BUILDING', 'REVIEWING',
                        'FINAL_REVIEW', 'SHADOW_READY', 'COMPLETED', 'BLOCKED',
                        'ESCALATED', 'ABANDONED'
                    )),
                    plan_version INTEGER NOT NULL DEFAULT 0,
                    remediation_round INTEGER NOT NULL DEFAULT 0,
                    pr_number INTEGER CHECK(pr_number IS NULL OR pr_number > 0),
                    current_pr_head TEXT CHECK(
                        current_pr_head IS NULL OR (
                            length(current_pr_head) = 40
                            AND current_pr_head NOT GLOB '*[^0-9a-f]*'
                        )
                    ),
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(repository, issue_number)
                );
                CREATE TABLE IF NOT EXISTS runs (
                    task_id TEXT PRIMARY KEY,
                    case_id TEXT NOT NULL REFERENCES cases(case_id),
                    role TEXT NOT NULL,
                    ordinal INTEGER NOT NULL CHECK(ordinal > 0),
                    payload_json TEXT NOT NULL,
                    payload_sha256 TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    UNIQUE(case_id, role, ordinal)
                );
                CREATE TABLE IF NOT EXISTS events (
                    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    case_id TEXT NOT NULL REFERENCES cases(case_id),
                    from_state TEXT,
                    to_state TEXT NOT NULL,
                    reason TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_events_case_event
                    ON events(case_id, event_id);
                CREATE TRIGGER IF NOT EXISTS runs_no_update
                BEFORE UPDATE ON runs
                BEGIN
                    SELECT RAISE(ABORT, 'immutable run records cannot be updated');
                END;
                CREATE TRIGGER IF NOT EXISTS runs_insert_guard
                BEFORE INSERT ON runs
                WHEN pip_case_write_authorized() != 1
                BEGIN
                    SELECT RAISE(ABORT, 'runs may only be appended by CaseStore');
                END;
                CREATE TRIGGER IF NOT EXISTS runs_no_delete
                BEFORE DELETE ON runs
                BEGIN
                    SELECT RAISE(ABORT, 'immutable run records cannot be deleted');
                END;
                CREATE TRIGGER IF NOT EXISTS events_no_update
                BEFORE UPDATE ON events
                BEGIN
                    SELECT RAISE(ABORT, 'append-only events cannot be updated');
                END;
                CREATE TRIGGER IF NOT EXISTS events_no_delete
                BEFORE DELETE ON events
                BEGIN
                    SELECT RAISE(ABORT, 'append-only events cannot be deleted');
                END;
                CREATE TRIGGER IF NOT EXISTS events_insert_guard
                BEFORE INSERT ON events
                WHEN pip_case_write_authorized() <> 1
                BEGIN
                    SELECT RAISE(ABORT, 'unauthorized case event insertion');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_no_delete
                BEFORE DELETE ON cases
                BEGIN
                    SELECT RAISE(ABORT, 'permanent cases cannot be deleted');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_identity_no_update
                BEFORE UPDATE OF case_id, repository, issue_number, workflow_version,
                    intake_label, created_at
                ON cases
                BEGIN
                    SELECT RAISE(ABORT, 'permanent case identity cannot be updated');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_insert_guard
                BEFORE INSERT ON cases
                WHEN pip_case_write_authorized() != 1
                BEGIN
                    SELECT RAISE(ABORT, 'cases may only be created by CaseStore');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_updated_at_guard
                BEFORE UPDATE OF updated_at ON cases
                WHEN OLD.updated_at <> NEW.updated_at
                     AND pip_case_write_authorized() <> 1
                BEGIN
                    SELECT RAISE(ABORT, 'unauthorized case timestamp update');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_transition_guard
                BEFORE UPDATE OF state ON cases
                WHEN OLD.state <> NEW.state AND (
                    pip_case_write_authorized() <> 1
                    OR NEW.updated_at = OLD.updated_at OR NOT (
                        (OLD.state = 'PLANNING' AND NEW.state IN (
                            'WAITING_HUMAN', 'BUILDING', 'COMPLETED',
                            'BLOCKED', 'ABANDONED'
                        )) OR
                        (OLD.state = 'WAITING_HUMAN' AND NEW.state IN (
                            'PLANNING', 'BLOCKED', 'ABANDONED'
                        )) OR
                        (OLD.state = 'BUILDING' AND NEW.state IN (
                            'REVIEWING', 'PLANNING', 'BLOCKED', 'ABANDONED'
                        )) OR
                        (OLD.state = 'REVIEWING' AND NEW.state IN (
                            'FINAL_REVIEW', 'BUILDING', 'ESCALATED',
                            'BLOCKED', 'ABANDONED'
                        )) OR
                        (OLD.state = 'FINAL_REVIEW' AND NEW.state IN (
                            'SHADOW_READY', 'COMPLETED', 'BUILDING', 'REVIEWING',
                            'PLANNING', 'WAITING_HUMAN', 'ESCALATED', 'BLOCKED',
                            'ABANDONED'
                        )) OR
                        (OLD.state = 'SHADOW_READY' AND NEW.state IN (
                            'COMPLETED', 'BLOCKED'
                        )) OR
                        (OLD.state = 'BLOCKED' AND NEW.state = 'PLANNING'
                        )
                    ) OR NOT EXISTS (
                        SELECT 1 FROM events
                        WHERE events.case_id = OLD.case_id
                          AND events.from_state = OLD.state
                          AND events.to_state = NEW.state
                          AND events.created_at = NEW.updated_at
                    )
                )
                BEGIN
                    SELECT RAISE(ABORT, 'unaudited or invalid case transition');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_remediation_guard
                BEFORE UPDATE OF remediation_round ON cases
                WHEN NEW.remediation_round <> OLD.remediation_round AND (
                    pip_case_write_authorized() <> 1 OR NOT (
                    OLD.state IN ('REVIEWING', 'FINAL_REVIEW')
                    AND NEW.state IN (
                        'BUILDING', 'REVIEWING', 'PLANNING', 'WAITING_HUMAN'
                    )
                    AND OLD.remediation_round < 3
                    AND NEW.remediation_round = OLD.remediation_round + 1
                    )
                )
                BEGIN
                    SELECT RAISE(ABORT, 'invalid remediation counter update');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_plan_guard
                BEFORE UPDATE OF plan_version ON cases
                WHEN NEW.plan_version <> OLD.plan_version AND (
                    pip_case_write_authorized() <> 1
                    OR NEW.updated_at = OLD.updated_at
                    OR OLD.state <> 'PLANNING'
                    OR NEW.plan_version <> OLD.plan_version + 1
                    OR NOT EXISTS (
                        SELECT 1 FROM events
                        WHERE events.case_id = OLD.case_id
                          AND events.from_state = OLD.state
                          AND events.to_state = OLD.state
                          AND events.created_at = NEW.updated_at
                          AND events.reason LIKE 'PLAN_VERSION:%'
                    )
                )
                BEGIN
                    SELECT RAISE(ABORT, 'invalid or unaudited plan version update');
                END;
                CREATE TRIGGER IF NOT EXISTS cases_pr_head_guard
                BEFORE UPDATE OF pr_number, current_pr_head ON cases
                WHEN (NEW.pr_number IS NOT OLD.pr_number
                      OR NEW.current_pr_head IS NOT OLD.current_pr_head)
                     AND (pip_case_write_authorized() <> 1
                     OR NEW.updated_at = OLD.updated_at OR NOT EXISTS (
                        SELECT 1 FROM events
                        WHERE events.case_id = OLD.case_id
                          AND events.from_state = OLD.state
                          AND events.to_state = OLD.state
                          AND events.created_at = NEW.updated_at
                          AND events.reason LIKE 'PR_HEAD:%'
                     ))
                BEGIN
                    SELECT RAISE(ABORT, 'unaudited PR head update');
                END;
                """
            )

    def create_case(
        self, case_id: str, repository: str, issue_number: int, intake_label: str
    ) -> None:
        if not self.ensure_case(case_id, repository, issue_number, intake_label):
            raise CaseStoreError(f"case already exists or conflicts: {case_id}")

    def ensure_case(
        self, case_id: str, repository: str, issue_number: int, intake_label: str
    ) -> bool:
        """Create once, or accept the same permanent identity for reconciliation."""
        if (
            not case_id
            or not repository
            or type(issue_number) is not int
            or issue_number < 1
        ):
            raise CaseStoreError("invalid permanent case identity")
        expected_case_id = f"{repository.rsplit('/', 1)[-1]}#{issue_number}"
        if case_id != expected_case_id:
            raise CaseStoreError(
                f"case ID {case_id!r} does not match repository issue {expected_case_id!r}"
            )
        if intake_label != "pip-ok":
            raise CaseStoreError("Pip v2 cases require the pip-ok intake label")
        now = _now()
        try:
            with self._connect() as db:
                with self._authorized_case_write():
                    inserted = db.execute(
                        """
                        INSERT OR IGNORE INTO cases(
                            case_id, repository, issue_number, intake_label, state,
                            created_at, updated_at
                        ) VALUES (?, ?, ?, ?, 'PLANNING', ?, ?)
                        """,
                        (case_id, repository, issue_number, intake_label, now, now),
                    )
                if inserted.rowcount == 1:
                    with self._authorized_case_write():
                        db.execute(
                            """
                            INSERT INTO events(
                                case_id, from_state, to_state, reason, created_at
                            ) VALUES (?, NULL, 'PLANNING', 'case-created', ?)
                            """,
                            (case_id, now),
                        )
                    return True
                row = db.execute(
                    """
                    SELECT repository, issue_number, intake_label
                    FROM cases WHERE case_id = ?
                    """,
                    (case_id,),
                ).fetchone()
                if row is None or (
                    row["repository"],
                    row["issue_number"],
                    row["intake_label"],
                ) != (repository, issue_number, intake_label):
                    raise CaseStoreError(f"case identity conflicts: {case_id}")
                return False
        except sqlite3.IntegrityError as exc:
            raise CaseStoreError(f"case identity conflicts: {case_id}") from exc

    def append_run(
        self,
        case_id: str,
        task_id: str,
        role: str,
        ordinal: int,
        payload: Mapping[str, Any],
    ) -> str:
        if payload.get("case_id") != case_id:
            raise CaseStoreError("run payload case_id does not match permanent case")
        if payload.get("task_id") != task_id:
            raise CaseStoreError("run payload task_id does not match immutable task ID")
        if payload.get("role") != role:
            raise CaseStoreError("run payload role does not match assigned role")
        serialized = _canonical(payload)
        digest = hashlib.sha256(serialized.encode()).hexdigest()
        try:
            with self._connect() as db:
                if (
                    db.execute(
                        "SELECT 1 FROM cases WHERE case_id = ?", (case_id,)
                    ).fetchone()
                    is None
                ):
                    raise CaseStoreError(f"unknown case: {case_id}")
                with self._authorized_case_write():
                    db.execute(
                        """
                        INSERT INTO runs(
                            task_id, case_id, role, ordinal, payload_json,
                            payload_sha256, created_at
                        ) VALUES (?, ?, ?, ?, ?, ?, ?)
                        """,
                        (task_id, case_id, role, ordinal, serialized, digest, _now()),
                    )
        except sqlite3.IntegrityError as exc:
            raise ImmutableRunError(f"immutable run already exists: {task_id}") from exc
        return digest

    def set_state(
        self,
        case_id: str,
        from_state: str,
        outcome: str,
        reason: str,
    ) -> CaseState:
        if from_state not in CASE_STATES or not outcome or not reason:
            raise CaseStoreError(
                "state transition requires source, outcome, and reason"
            )
        with self._connect() as db:
            row = db.execute(
                "SELECT state, remediation_round, updated_at FROM cases WHERE case_id = ?",
                (case_id,),
            ).fetchone()
            if row is None:
                raise CaseStoreError(f"unknown case: {case_id}")
            if row["state"] != from_state:
                raise CaseStoreError(
                    f"stale state transition for {case_id}: expected {from_state}, "
                    f"found {row['state']}"
                )
            now = _fresh_now(row["updated_at"])
            current_round = int(row["remediation_round"])
            try:
                to_state = transition(
                    CaseState(from_state),
                    outcome,
                    remediation_round=current_round,
                    max_rounds=MAX_REMEDIATION_ROUNDS,
                )
            except (ValueError, TransitionError) as exc:
                raise CaseStoreError(str(exc)) from exc
            next_round = current_round
            final_returns = {
                "RETURN_TO_BUILD",
                "RETURN_TO_REVIEW",
                "RETURN_TO_PLANNING",
                "WAIT_FOR_ISSUE_CREATOR",
            }
            if (outcome == "REQUEST_CHANGES" and to_state is CaseState.BUILDING) or (
                from_state == CaseState.FINAL_REVIEW.value
                and outcome in final_returns
                and to_state is not CaseState.ESCALATED
            ):
                next_round += 1
            with self._authorized_case_write():
                db.execute(
                    """
                    INSERT INTO events(case_id, from_state, to_state, reason, created_at)
                    VALUES (?, ?, ?, ?, ?)
                    """,
                    (
                        case_id,
                        from_state,
                        to_state.value,
                        f"{outcome}: {reason}",
                        now,
                    ),
                )
            with self._authorized_case_write():
                updated = db.execute(
                    """
                    UPDATE cases
                    SET state = ?, remediation_round = ?, updated_at = ?
                    WHERE case_id = ? AND state = ?
                    """,
                    (to_state.value, next_round, now, case_id, from_state),
                )
            if updated.rowcount != 1:
                row = db.execute(
                    "SELECT state FROM cases WHERE case_id = ?", (case_id,)
                ).fetchone()
                if row is None:
                    raise CaseStoreError(f"unknown case: {case_id}")
                raise CaseStoreError(
                    f"stale state transition for {case_id}: expected {from_state}, "
                    f"found {row['state']}"
                )

        return to_state

    def record_plan_version(
        self, case_id: str, expected_version: int, new_version: int
    ) -> None:
        if (
            type(expected_version) is not int
            or type(new_version) is not int
            or new_version != expected_version + 1
        ):
            raise CaseStoreError("plan version must advance by exactly one")
        with self._connect() as db:
            row = db.execute(
                "SELECT state, plan_version, updated_at FROM cases WHERE case_id = ?",
                (case_id,),
            ).fetchone()
            if row is None:
                raise CaseStoreError(f"unknown case: {case_id}")
            if row["state"] != CaseState.PLANNING.value:
                raise CaseStoreError("plans can only advance while planning")
            if row["plan_version"] != expected_version:
                raise CaseStoreError("stale plan version")
            now = _fresh_now(row["updated_at"])
            with self._authorized_case_write():
                db.execute(
                    """
                    INSERT INTO events(case_id, from_state, to_state, reason, created_at)
                    VALUES (?, ?, ?, ?, ?)
                    """,
                    (
                        case_id,
                        row["state"],
                        row["state"],
                        f"PLAN_VERSION: {expected_version} -> {new_version}",
                        now,
                    ),
                )
            with self._authorized_case_write():
                db.execute(
                    """
                    UPDATE cases SET plan_version = ?, updated_at = ?
                    WHERE case_id = ? AND plan_version = ?
                    """,
                    (new_version, now, case_id, expected_version),
                )

    def bind_pr_head(
        self,
        case_id: str,
        pr_number: int,
        new_head: str,
        *,
        expected_head: str | None = None,
    ) -> None:
        if type(pr_number) is not int or pr_number < 1:
            raise CaseStoreError("invalid PR number")
        if len(new_head) != 40 or any(
            char not in "0123456789abcdef" for char in new_head
        ):
            raise CaseStoreError("invalid PR head")
        with self._connect() as db:
            row = db.execute(
                """
                SELECT state, pr_number, current_pr_head, updated_at
                FROM cases WHERE case_id = ?
                """,
                (case_id,),
            ).fetchone()
            if row is None:
                raise CaseStoreError(f"unknown case: {case_id}")
            if row["pr_number"] not in (None, pr_number):
                raise CaseStoreError("case is already bound to another PR")
            if row["current_pr_head"] != expected_head:
                raise CaseStoreError("stale PR head")
            now = _fresh_now(row["updated_at"])
            with self._authorized_case_write():
                db.execute(
                    """
                    INSERT INTO events(case_id, from_state, to_state, reason, created_at)
                    VALUES (?, ?, ?, ?, ?)
                    """,
                    (
                        case_id,
                        row["state"],
                        row["state"],
                        f"PR_HEAD: PR {pr_number} at {new_head}",
                        now,
                    ),
                )
            with self._authorized_case_write():
                db.execute(
                    """
                    UPDATE cases SET pr_number = ?, current_pr_head = ?, updated_at = ?
                    WHERE case_id = ?
                      AND pr_number IS ?
                      AND current_pr_head IS ?
                    """,
                    (
                        pr_number,
                        new_head,
                        now,
                        case_id,
                        row["pr_number"],
                        expected_head,
                    ),
                )

    def get_case(self, case_id: str) -> dict[str, Any]:
        with self._connect() as db:
            row = db.execute(
                "SELECT * FROM cases WHERE case_id = ?", (case_id,)
            ).fetchone()
        if row is None:
            raise CaseStoreError(f"unknown case: {case_id}")
        return dict(row)

    def list_runs(self, case_id: str) -> list[dict[str, Any]]:
        with self._connect() as db:
            rows = db.execute(
                """
                SELECT task_id, case_id, role, ordinal, payload_json,
                       payload_sha256, created_at
                FROM runs WHERE case_id = ? ORDER BY rowid
                """,
                (case_id,),
            ).fetchall()
        result: list[dict[str, Any]] = []
        for row in rows:
            item = dict(row)
            item["payload"] = json.loads(item.pop("payload_json"))
            result.append(item)
        return result
