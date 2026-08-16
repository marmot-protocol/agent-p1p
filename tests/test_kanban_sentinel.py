from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from pip_agent.kanban_sentinel import SentinelError, create_sticky_sentinel


def _database(tmp_path: Path) -> Path:
    database = tmp_path / ".hermes/kanban/boards/pip-mdk/kanban.db"
    database.parent.mkdir(parents=True)
    connection = sqlite3.connect(database)
    connection.executescript(
        """
        CREATE TABLE tasks(
          id TEXT PRIMARY KEY, title TEXT NOT NULL, body TEXT, assignee TEXT,
          status TEXT NOT NULL, priority INTEGER DEFAULT 0, created_by TEXT,
          created_at INTEGER NOT NULL, workspace_kind TEXT NOT NULL DEFAULT 'scratch',
          idempotency_key TEXT, block_kind TEXT,
          block_recurrences INTEGER NOT NULL DEFAULT 0
        );
        CREATE UNIQUE INDEX task_idempotency ON tasks(idempotency_key);
        CREATE TABLE task_events(
          id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL,
          run_id INTEGER, kind TEXT NOT NULL, payload TEXT, created_at INTEGER NOT NULL
        );
        """
    )
    connection.close()
    return database


def test_sticky_sentinel_is_inserted_atomically_and_idempotently(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    database = _database(tmp_path)
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))

    first = create_sticky_sentinel("pip-mdk", "route-1")
    second = create_sticky_sentinel("pip-mdk", "route-1")
    assert first == second

    connection = sqlite3.connect(database)
    task = connection.execute(
        "SELECT status,assignee,block_kind FROM tasks WHERE id=?", (first,)
    ).fetchone()
    events = connection.execute(
        "SELECT kind FROM task_events WHERE task_id=? ORDER BY id", (first,)
    ).fetchall()
    assert task == ("blocked", None, "needs_input")
    assert events == [("created",), ("blocked",)]
    assert connection.execute("SELECT count(*) FROM tasks").fetchone()[0] == 1


def test_existing_nonsticky_sentinel_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    database = _database(tmp_path)
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    task_id = create_sticky_sentinel("pip-mdk", "route-1")
    connection = sqlite3.connect(database)
    connection.execute(
        "INSERT INTO task_events(task_id,run_id,kind,payload,created_at) VALUES(?,NULL,'unblocked','{}',1)",
        (task_id,),
    )
    connection.commit()
    connection.close()

    with pytest.raises(SentinelError, match="not sticky-blocked"):
        create_sticky_sentinel("pip-mdk", "route-1")
