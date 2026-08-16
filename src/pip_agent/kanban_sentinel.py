from __future__ import annotations

import hashlib
import json
import os
import re
import sqlite3
import time
from pathlib import Path


class SentinelError(RuntimeError):
    pass


_BOARD_RE = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}")


def create_sticky_sentinel(board: str, route_id: str) -> str:
    if _BOARD_RE.fullmatch(board) is None:
        raise SentinelError("invalid Kanban board slug")
    home = Path(os.environ.get("HERMES_HOME", "~/.hermes")).expanduser().resolve()
    board_dir = home / "kanban" / "boards" / board
    database = board_dir / "kanban.db"
    if board_dir.is_symlink() or database.is_symlink() or not database.is_file():
        raise SentinelError("Kanban board database is missing or unsafe")
    if database.resolve().parent != board_dir.resolve():
        raise SentinelError("Kanban board database escaped its board directory")

    idempotency_key = f"pip-v2:{route_id}:gate-sentinel"
    task_id = "t_" + hashlib.sha256(idempotency_key.encode()).hexdigest()[:16]
    now = int(time.time())
    created_payload = json.dumps(
        {
            "assignee": None,
            "status": "blocked",
            "parents": [],
            "workspace_kind": "scratch",
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    blocked_payload = json.dumps(
        {"reason": "permanent Pip v2 activation-gate sentinel", "kind": "needs_input"},
        sort_keys=True,
        separators=(",", ":"),
    )

    connection: sqlite3.Connection | None = None
    try:
        connection = sqlite3.connect(database, timeout=10)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA busy_timeout=10000")
        connection.execute("BEGIN IMMEDIATE")
        columns = {
            row[1] for row in connection.execute("PRAGMA table_info(tasks)").fetchall()
        }
        required = {
            "id",
            "title",
            "body",
            "assignee",
            "status",
            "priority",
            "created_by",
            "created_at",
            "workspace_kind",
            "idempotency_key",
            "block_kind",
            "block_recurrences",
        }
        if not required <= columns:
            raise SentinelError("Kanban task schema is incompatible")
        event_columns = {
            row[1]
            for row in connection.execute("PRAGMA table_info(task_events)").fetchall()
        }
        if not {"task_id", "run_id", "kind", "payload", "created_at"} <= event_columns:
            raise SentinelError("Kanban event schema is incompatible")

        existing = connection.execute(
            "SELECT id, status FROM tasks WHERE idempotency_key = ?",
            (idempotency_key,),
        ).fetchone()
        if existing is None:
            collision = connection.execute(
                "SELECT 1 FROM tasks WHERE id = ?", (task_id,)
            ).fetchone()
            if collision is not None:
                raise SentinelError("deterministic sentinel task ID collided")
            connection.execute(
                """
                INSERT INTO tasks(
                    id,title,body,assignee,status,priority,created_by,created_at,
                    workspace_kind,idempotency_key,block_kind,block_recurrences
                ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)
                """,
                (
                    task_id,
                    "Pip v2 activation-gate sentinel",
                    f"Permanent sticky sentinel for route {route_id}.",
                    None,
                    "blocked",
                    0,
                    "pip-v2-router",
                    now,
                    "scratch",
                    idempotency_key,
                    "needs_input",
                    0,
                ),
            )
            connection.execute(
                "INSERT INTO task_events(task_id,run_id,kind,payload,created_at) VALUES(?,?,?,?,?)",
                (task_id, None, "created", created_payload, now),
            )
            connection.execute(
                "INSERT INTO task_events(task_id,run_id,kind,payload,created_at) VALUES(?,?,?,?,?)",
                (task_id, None, "blocked", blocked_payload, now),
            )
        else:
            task_id = existing["id"]
            latest = connection.execute(
                """
                SELECT kind FROM task_events
                 WHERE task_id = ? AND kind IN ('blocked','unblocked')
                 ORDER BY id DESC LIMIT 1
                """,
                (task_id,),
            ).fetchone()
            if (
                existing["status"] != "blocked"
                or latest is None
                or latest["kind"] != "blocked"
            ):
                raise SentinelError(
                    "existing activation sentinel is not sticky-blocked"
                )
        connection.commit()
    except (sqlite3.Error, OSError) as exc:
        raise SentinelError("atomic activation sentinel creation failed") from exc
    finally:
        if connection is not None:
            connection.close()
    return task_id
