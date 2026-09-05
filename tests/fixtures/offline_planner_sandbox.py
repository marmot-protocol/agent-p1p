"""No-model planner lifecycle fixture. Run only inside the bounded OS sandbox.

The worker is deterministic test code, not a model. Stock Hermes owns queue,
claim and completion; the untouched CLI result is then checked by Rust.
"""
import json
import os
import pathlib
import subprocess
import sys
import time

source, scratch, result_template = map(pathlib.Path, sys.argv[1:])
assert source.is_absolute() and scratch.is_absolute()
assert source.resolve() == source and scratch.resolve() == scratch
assert (scratch / "OFFLINE_PROBE").read_text() == "pip-offline-planner-sandbox\n"
assert os.stat(scratch).st_dev != os.stat("/").st_dev
assert os.statvfs(scratch).f_bavail * os.statvfs(scratch).f_frsize >= 536870912000
for path in (source / "src/lib.rs", pathlib.Path("/var/lib/pip/ledger.db"), pathlib.Path("/var/lib/pip/provider-home"), pathlib.Path("/var/lib/pip/hermes/auth.json")):
    if path == source / "src/lib.rs":
        try:
            with path.open("a"):
                raise AssertionError("source unexpectedly writable")
        except PermissionError:
            pass
        except OSError as error:
            assert error.errno == 30, error
    else:
        assert not os.access(path, os.R_OK), f"private path readable: {path}"

home = scratch / "hermes"
for child in (home, home / "profiles/planner", scratch / "disposable/target", scratch / "disposable/cargo-home", scratch / "disposable/tmp", scratch / "results"):
    child.mkdir(parents=True, exist_ok=True)
    child.chmod(0o700)
for name in ("HOME", "HERMES_HOME", "HERMES_KANBAN_HOME"):
    os.environ[name] = str(home)
os.environ.update(CARGO_HOME=str(scratch / "disposable/cargo-home"), CARGO_TARGET_DIR=str(scratch / "disposable/target"), TMPDIR=str(scratch / "disposable/tmp"))
configuration = {"lsp":{"enabled":False}, "terminal":{"backend":"local", "home_mode":"profile"}}
(home / "config.yaml").write_text(json.dumps(configuration))
(home / "profiles/planner/config.yaml").write_text(json.dumps(configuration))
from hermes_cli import kanban_db as kb
from hermes_cli.config import load_config_readonly
assert load_config_readonly()["lsp"]["enabled"] is False
from agent.lsp import get_service
assert get_service() is None, "optional language-server service unexpectedly active"

board = "pip-offline-planner-sandbox"
projection_key = "repo:984321#1240@1:planner:round:1:revision:1:worker"
started = int(time.time())
with kb.connect_closing(board=board) as conn:
    task_id = kb.create_task(conn, title="Run planner", body=json.dumps({"projection_key":projection_key,"fixture_only":True}), assignee="planner", created_by="pip-controller", workspace_kind="dir", workspace_path=str(source), max_retries=1, initial_status="blocked", board=board)
    if kb.get_task(conn, task_id).status == "blocked":
        assert kb.unblock_task(conn, task_id)
    spawns = []

    def offline_worker(task, workspace_path, board=None):
        assert pathlib.Path(workspace_path) == source
        spawns.append(task.id)
        output = subprocess.run(["/usr/local/bin/cargo", "test", "--locked", "--offline", "--manifest-path", str(source / "Cargo.toml"), "-j", "2"], cwd=source, capture_output=True, text=True, timeout=120)
        (scratch / "results/cargo.log").write_text(output.stdout + output.stderr)
        assert output.returncode == 0, output.stderr
        metadata = json.loads(result_template.read_text())["results"][0]
        metadata.update(task_id=task.id, requested_model="openai-codex/gpt-6-astra", actual_model="openai-codex/gpt-6-astra", started_at_unix=started, completed_at_unix=int(time.time()))
        artifact = scratch / "results/plan.md"
        artifact.write_text("# Synthetic offline planner fixture\n\nNo model was called. Cargo passed in the assigned disposable directory. Source remained read-only. This is a transport and sandbox test, not a canary plan.\n")
        metadata["plan_artifact"] = str(artifact)
        metadata["evidence"] = {"synthetic_offline":True,"provider_calls":0,"cargo_log":str(scratch / "results/cargo.log"),"plan_markdown":artifact.read_text()}
        assert kb.complete_task(conn, task.id, metadata=metadata, fire_lifecycle_hook=False)
        return None

    kb.dispatch_once(conn, board=board, spawn_fn=offline_worker, max_spawn=1)
    assert spawns == [task_id]
    assert kb.get_task(conn, task_id).status == "done"
    shown = subprocess.run(["/usr/local/bin/hermes", "kanban", "--board", board, "show", task_id, "--json"], check=True, capture_output=True, text=True, timeout=30)
    report = {"ok":True,"provider_calls":0,"task_detail":json.loads(shown.stdout)}
    (scratch / "results/report.json").write_text(json.dumps(report))
    print("PIP_OFFLINE_PLANNER_SANDBOX_OK")
