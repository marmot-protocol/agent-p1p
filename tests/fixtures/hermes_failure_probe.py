"""Offline stock-Hermes adapter probe; never connects to a model or live board.

Run with the installed Hermes virtualenv Python and its source on PYTHONPATH.
Only process observations are simulated; the dispatcher and database are stock.
"""

import json
import os
import pathlib
import tempfile
import subprocess
from unittest.mock import patch


with tempfile.TemporaryDirectory(prefix="pip-hermes-failure-probe-") as temporary:
    root = pathlib.Path(temporary)
    for name in ("HOME", "HERMES_HOME", "HERMES_KANBAN_HOME"):
        os.environ[name] = str(root)

    from hermes_cli import kanban_db as kb
    from hermes_cli.profiles import get_profile_dir

    get_profile_dir("planner").mkdir(parents=True, exist_ok=True)
    reports = []
    terminal_detail = None
    for limit in (3, 1):
        board = f"pip-offline-failure-{limit}"
        workspace = root / f"workspace-{limit}"
        workspace.mkdir()
        with kb.connect_closing(board=board) as conn:
            task_id = kb.create_task(
                conn,
                title="Offline clean-exit protocol failure",
                body="{}",
                assignee="planner",
                created_by="pip-offline-test",
                workspace_kind="dir",
                workspace_path=str(workspace),
                max_retries=limit,
                initial_status="blocked",
                board=board,
            )
            if kb.get_task(conn, task_id).status == "blocked":
                assert kb.unblock_task(conn, task_id)
            assert kb.get_task(conn, task_id).status == "ready"
            spawned = []

            def fake_worker(task, workspace_path, board=None):
                assert task.id == task_id
                spawned.append(task.id)
                return os.getpid()

            kb.dispatch_once(conn, board=board, spawn_fn=fake_worker, max_spawn=1)
            assert len(spawned) == 1
            assert kb.get_task(conn, task_id).status == "running"
            # No subprocess is created or killed. Simulate the observed clean
            # exit without kanban_complete/block, after the launch grace period.
            with (
                patch.object(kb, "_pid_alive", return_value=False),
                patch.object(kb, "_classify_worker_exit", return_value=("clean_exit", 0)),
                patch.object(kb, "_resolve_crash_grace_seconds", return_value=0),
            ):
                assert kb.detect_crashed_workers(conn) == [task_id]
            status = kb.get_task(conn, task_id).status
            assert status == ("blocked" if limit == 1 else "ready"), status
            if limit == 1:
                for _ in range(2):
                    kb.dispatch_once(conn, board=board, spawn_fn=fake_worker, max_spawn=1)
                assert len(spawned) == 1, "failed worker was retried"
                runs = kb.list_runs(conn, task_id)
                assert runs[-1].outcome == "crashed", runs
                assert kb.list_events(conn, task_id)[-1].kind == "gave_up"
                executable = os.environ.get("PIP_TEST_HERMES", "/usr/local/bin/hermes")
                shown = subprocess.run(
                    [executable, "kanban", "--board", board, "show", task_id, "--json"],
                    check=True, capture_output=True, text=True, timeout=30,
                )
                terminal_detail = json.loads(shown.stdout)
                assert terminal_detail["task"]["id"] == task_id
                assert terminal_detail["task"]["status"] == "blocked"
                assert terminal_detail["events"][-1]["kind"] == "gave_up"
            reports.append({"max_attempts": limit, "after_failure": status, "spawns": len(spawned)})
    print(json.dumps({"ok": True, "provider_calls": 0, "probes": reports, "terminal_detail": terminal_detail}))
