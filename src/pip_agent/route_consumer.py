from __future__ import annotations

import argparse
import json
import os
import signal
import stat
import subprocess
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

from .github_gate import (
    fetch_live_final_evidence,
    validate_live_final_snapshot,
    validate_live_implementation_base,
)
from .kanban_gate import advance_gates
from .kanban_router import RouteError, canonical_route_id, route_once, validate_route

MAX_ROUTE_BYTES = 65_536
PASSIVE_ACTIONS = {"wait", "stop", "continue", "request_explicit_command"}

ROUTE_CONSUMER_SERVICE = """[Unit]
Description=Pip v2 exact-canary Kanban router
After=network-online.target pip-v2-control.service
Wants=network-online.target
ConditionPathExists=/run/pip-v2/decision-route.json

[Service]
Type=oneshot
User=@CALLER@
Group=@CALLER_GROUP@
LoadCredential=github.token:/etc/pip-v2/github.token
Environment=HOME=@CALLER_HOME@
Environment=PATH=@CALLER_HOME@/.local/bin:/usr/local/bin:/usr/bin:/bin
Environment=PYTHONPATH=/opt/pip-v2/current
Environment=PYTHONDONTWRITEBYTECODE=1
Environment=PYTHONSAFEPATH=1
ExecStart=/usr/local/bin/pip-v2-route-consumer consume --route /run/pip-v2/decision-route.json --board pip-mdk --skills-repository-commit-file /opt/pip-v2/SOURCE.COMMIT
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=false
ReadWritePaths=@CALLER_HOME@/.hermes
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
ProtectHostname=true
RestrictSUIDSGID=true
LockPersonality=true
RestrictRealtime=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
SystemCallArchitectures=native
"""

ROUTE_CONSUMER_TIMER = """[Unit]
Description=Poll Pip v2 exact-canary decision route

[Timer]
OnActiveSec=15s
OnUnitActiveSec=15s
AccuracySec=1s
Persistent=true
Unit=pip-v2-route-consumer.service

[Install]
WantedBy=timers.target
"""


class RouteConsumerError(RouteError):
    """A protected decision route could not be routed to Kanban."""


def render_route_consumer_service(caller: str, caller_group: str, home: Path) -> str:
    safe = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-"
    if any(
        not value or any(character not in safe for character in value)
        for value in (caller, caller_group)
    ):
        raise RouteConsumerError("unsafe route-consumer identity")
    if not home.is_absolute() or " " in str(home):
        raise RouteConsumerError("unsafe route-consumer home")
    return (
        ROUTE_CONSUMER_SERVICE.replace("@CALLER@", caller)
        .replace("@CALLER_GROUP@", caller_group)
        .replace("@CALLER_HOME@", str(home))
    )


def render_route_consumer_timer() -> str:
    return ROUTE_CONSUMER_TIMER


def _read_source_commit(path: Path) -> str:
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != 0
            or stat.S_IMODE(metadata.st_mode) & 0o022
            or metadata.st_size not in {40, 41}
        ):
            raise RouteConsumerError("skills repository commit file is untrusted")
        raw = os.read(descriptor, 42)
    finally:
        os.close(descriptor)
    commit = raw.decode("ascii").strip()
    if len(commit) != 40 or any(char not in "0123456789abcdef" for char in commit):
        raise RouteConsumerError("skills repository commit file is malformed")
    return commit


def _read_route(path: Path) -> dict[str, Any]:
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise RouteConsumerError("decision route is not a regular file")
        if stat.S_IMODE(metadata.st_mode) != 0o640:
            raise RouteConsumerError("decision route has unsafe mode")
        raw = os.read(descriptor, MAX_ROUTE_BYTES + 1)
        if len(raw) > MAX_ROUTE_BYTES:
            raise RouteConsumerError("decision route exceeds size limit")
    finally:
        os.close(descriptor)
    try:
        payload = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RouteConsumerError("decision route is malformed") from exc
    if not isinstance(payload, dict):
        raise RouteConsumerError("decision route must be an object")
    return payload


def _default_runner(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=False, capture_output=True, text=True)


def _list_canary_tasks(
    board: str, runner: Callable[[list[str]], subprocess.CompletedProcess[str]]
) -> list[dict[str, Any]]:
    listed = runner(
        ["hermes", "kanban", "--board", board, "list", "--archived", "--json"]
    )
    if listed.returncode != 0:
        raise RouteConsumerError("cannot list Pip v2 Kanban tasks")
    try:
        payload = json.loads(listed.stdout)
    except json.JSONDecodeError as exc:
        raise RouteConsumerError("Kanban task listing is malformed") from exc
    tasks = payload.get("tasks") if isinstance(payload, dict) else payload
    if not isinstance(tasks, list):
        raise RouteConsumerError("Kanban task listing is malformed")
    return [task for task in tasks if isinstance(task, dict)]


def _worker_pid(
    board: str,
    task_id: str,
    runner: Callable[[list[str]], subprocess.CompletedProcess[str]],
) -> int | None:
    shown = runner(["hermes", "kanban", "--board", board, "show", task_id, "--json"])
    if shown.returncode != 0:
        raise RouteConsumerError("cannot inspect running Pip v2 task")
    try:
        payload = json.loads(shown.stdout)
    except json.JSONDecodeError as exc:
        raise RouteConsumerError("Kanban running-task response is malformed") from exc
    runs = payload.get("runs") if isinstance(payload, dict) else None
    if not isinstance(runs, list) or not runs or not isinstance(runs[-1], dict):
        return None
    pid = runs[-1].get("worker_pid")
    return pid if type(pid) is int and pid > 1 else None


def _process_start(pid: int) -> str | None:
    try:
        if Path(f"/proc/{pid}").stat().st_uid != os.geteuid():
            return None
        return Path(f"/proc/{pid}/stat").read_text().split()[21]
    except (FileNotFoundError, IndexError, OSError):
        return None


def _descendants(root: int) -> list[int]:
    parents: dict[int, int] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text().split()
            parents[int(entry.name)] = int(fields[3])
        except (FileNotFoundError, IndexError, OSError, ValueError):
            continue
    found: list[int] = []
    frontier = [root]
    while frontier:
        parent = frontier.pop()
        children = [pid for pid, ppid in parents.items() if ppid == parent]
        found.extend(children)
        frontier.extend(children)
    return found


def _terminate_worker(pid: int) -> None:
    start = _process_start(pid)
    if start is None:
        return
    targets = [*_descendants(pid), pid]
    identities = {target: _process_start(target) for target in targets}
    for target in [*reversed(targets[:-1]), pid]:
        if identities[target] is not None:
            try:
                os.kill(target, signal.SIGTERM)
            except ProcessLookupError:
                pass
    deadline = time.monotonic() + 10
    survivors: list[int] = []
    while time.monotonic() < deadline:
        survivors = [
            target
            for target, identity in identities.items()
            if identity is not None and _process_start(target) == identity
        ]
        if not survivors:
            return
        time.sleep(0.1)
    for target in survivors:
        if _process_start(target) == identities[target]:
            try:
                os.kill(target, signal.SIGKILL)
            except ProcessLookupError:
                pass
    time.sleep(0.1)
    if any(
        identity is not None and _process_start(target) == identity
        for target, identity in identities.items()
    ):
        raise RouteConsumerError("Pip v2 worker did not terminate")


def _archive_canary_tasks(
    board: str,
    runner: Callable[[list[str]], subprocess.CompletedProcess[str]],
    terminator: Callable[[int], None] = _terminate_worker,
    *,
    preserve_route_id: str | None = None,
) -> list[str]:
    tasks = _list_canary_tasks(board, runner)
    terminal = {"archived"}
    task_ids = [
        task["id"]
        for task in tasks
        if isinstance(task, dict)
        and task.get("created_by") == "pip-v2-router"
        and isinstance(task.get("body"), str)
        and '"case_id": "mdk#1240"' in task["body"]
        and (
            preserve_route_id is None
            or f'"route_id": "{preserve_route_id}"' not in task["body"]
        )
        and task.get("status") not in terminal
        and isinstance(task.get("id"), str)
    ]
    if not task_ids:
        return []
    for task in tasks:
        if task.get("id") in task_ids and task.get("status") == "running":
            pid = _worker_pid(board, task["id"], runner)
            if pid is None:
                raise RouteConsumerError("running Pip v2 task has no worker PID")
            terminator(pid)
    command = [
        "hermes",
        "kanban",
        "--board",
        board,
        "archive",
        *task_ids,
    ]
    archived = runner(command)
    if archived.returncode != 0:
        raise RouteConsumerError("cannot archive superseded Pip v2 Kanban tasks")
    return task_ids


def _validate_live_route(route: dict[str, Any]) -> None:
    from .decision_reconciler import (
        PLANNER_ACTOR,
        fetch_canary_comments,
        fetch_canary_issue_authorization,
        parse_human_decision,
        parse_planner_evidence,
    )

    fetch_canary_issue_authorization()
    comments = fetch_canary_comments()
    if route.get("decision") == "automatic":
        bound_comment_id = route.get("planner_comment_id")
        bound_comments = [
            comment
            for comment in comments
            if not isinstance(comment, dict)
            or not isinstance(comment.get("user"), dict)
            or comment["user"].get("login") != PLANNER_ACTOR
            or comment.get("id") == bound_comment_id
        ]
        planner = parse_planner_evidence(bound_comments)
        decision = parse_human_decision(bound_comments)
    else:
        planner = parse_planner_evidence(comments)
        decision = parse_human_decision(comments)
    if route.get("decision") == "automatic":
        if decision is not None and decision.kind in {"reject", "narrow"}:
            raise RouteConsumerError(
                "live human decision supersedes automatic planner disposition"
            )
        if planner.outcome != "PROCEED":
            raise RouteConsumerError(
                f"live planner no longer permits automatic work: outcome={planner.outcome!r}"
            )
        expected = {
            "comment_id": planner.comment_id,
            "comment_url": planner.comment_url,
            "evidence_body_sha256": planner.body_sha256,
            "planner_outcome": planner.outcome,
            "planner_task_id": planner.execution_task_id,
            "authorized_scope": planner.authorized_scope,
            "narrowed_scope": planner.narrowed_scope,
        }
    else:
        if decision is None or decision.kind != route.get("decision"):
            observed = None if decision is None else decision.kind
            raise RouteConsumerError(
                f"live human decision no longer matches route: decision={observed!r}"
            )
        expected = {
            "comment_id": decision.comment_id,
            "comment_url": decision.comment_url,
            "evidence_body_sha256": decision.body_sha256,
            "narrowed_scope": decision.narrowed_scope or planner.narrowed_scope,
        }
    expected.update(
        {
            "planner_comment_id": planner.comment_id,
            "planner_comment_url": planner.comment_url,
            "planner_body_sha256": planner.body_sha256,
            "plan_version": planner.plan_version,
            "planned_base_sha": planner.planned_base_sha,
        }
    )
    mismatches = [key for key, value in expected.items() if route.get(key) != value]
    if mismatches:
        raise RouteConsumerError(
            "live human decision no longer matches route: " + ", ".join(mismatches)
        )


def consume_route(
    route: dict[str, Any],
    *,
    board: str,
    runner: Callable[[list[str]], subprocess.CompletedProcess[str]] = _default_runner,
    live_validator: Callable[[dict[str, Any]], None] = _validate_live_route,
    gate_advancer: Callable[..., dict[str, Any]] = advance_gates,
    terminator: Callable[[int], None] = _terminate_worker,
    skills_repository_commit: str | None = None,
) -> dict[str, Any]:
    action = route.get("action")
    if action == "stop" and route.get("case_id") == "mdk#1240":
        return {
            "status": "stopped",
            "action": action,
            "archived_tasks": _archive_canary_tasks(board, runner, terminator),
        }
    if action in PASSIVE_ACTIONS or route.get("ok") is not True:
        return {"status": "passive", "action": action}

    def checked_live() -> None:
        from .decision_reconciler import ExternalDecisionError

        try:
            live_validator(route)
        except ExternalDecisionError:
            # Fail closed for this activation without destroying already-running,
            # independently authorized work on a transient lookup failure.
            raise
        except Exception:
            _archive_canary_tasks(board, runner, terminator)
            raise

    if action == "replan":
        _archive_canary_tasks(
            board,
            runner,
            terminator,
            preserve_route_id=str(route.get("route_id")),
        )
    route_kwargs: dict[str, Any] = {
        "board": board,
        "runner": runner,
        "before_activate": checked_live,
    }
    if skills_repository_commit is not None:
        route_kwargs["skills_repository_commit"] = skills_repository_commit
    task_ids = route_once(route, **route_kwargs)
    result: dict[str, Any] = {"status": "routed", "action": action, "tasks": task_ids}
    if action == "dispatch_builder":
        result["gate"] = gate_advancer(
            route,
            task_ids,
            board=board,
            runner=runner,
            before_activate=checked_live,
            implementation_base_validator=lambda base, head: (
                validate_live_implementation_base(base, head, runner=runner)
            ),
            final_evidence_validator=lambda pr, head: fetch_live_final_evidence(
                pr, head, runner=runner
            ),
            final_snapshot_validator=lambda pr, head: validate_live_final_snapshot(
                pr, head, runner=runner
            ),
        )
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Route Pip v2 decisions into Hermes Kanban"
    )
    commands = parser.add_subparsers(dest="command", required=True)
    consume = commands.add_parser("consume")
    consume.add_argument("--route", type=Path, required=True)
    consume.add_argument("--board", default="pip-mdk")
    consume.add_argument("--skills-repository-commit-file", type=Path, required=True)
    render = commands.add_parser("render-units")
    render.add_argument("--caller", required=True)
    render.add_argument("--caller-group", required=True)
    render.add_argument("--caller-home", type=Path, required=True)
    render.add_argument("--service-output", type=Path, required=True)
    render.add_argument("--timer-output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "render-units":
            args.service_output.write_text(
                render_route_consumer_service(
                    args.caller, args.caller_group, args.caller_home
                )
            )
            args.timer_output.write_text(render_route_consumer_timer())
            return 0
        result = consume_route(
            _read_route(args.route),
            board=args.board,
            skills_repository_commit=_read_source_commit(
                args.skills_repository_commit_file
            ),
        )
        print(json.dumps(result, sort_keys=True))
        return 0
    except (OSError, RouteError, subprocess.SubprocessError) as exc:
        print(str(exc), file=sys.stderr)
        return 1


__all__ = [
    "RouteConsumerError",
    "canonical_route_id",
    "consume_route",
    "main",
    "render_route_consumer_service",
    "render_route_consumer_timer",
    "validate_route",
]


if __name__ == "__main__":
    raise SystemExit(main())
