from __future__ import annotations

import hashlib
import json
import subprocess
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

from .kanban_sentinel import SentinelError, create_sticky_sentinel


class RouteError(RuntimeError):
    """A decision route is malformed or inconsistent."""


CASE_ID = "mdk#1240"
REPOSITORY = "marmot-protocol/mdk"
ISSUE_NUMBER = 1240
ISSUE_URL = "https://github.com/marmot-protocol/mdk/issues/1240"
ACTIVE_ACTIONS = {"dispatch_builder", "replan"}


@dataclass(frozen=True)
class TaskSpec:
    key: str
    title: str
    body: str
    assignee: str
    parents: tuple[str, ...]
    skills: tuple[str, ...]
    max_runtime: str
    model: str | None = None
    provider: str | None = None


def canonical_route_id(payload: Mapping[str, Any]) -> str:
    bound = {key: value for key, value in payload.items() if key != "route_id"}
    encoded = json.dumps(
        bound, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode()
    return "decision-" + hashlib.sha256(encoded).hexdigest()


def validate_route(payload: Mapping[str, Any]) -> str:
    if payload.get("route_id") != canonical_route_id(payload):
        raise RouteError("route ID does not bind the complete payload")
    if (
        payload.get("ok") is not True
        or payload.get("case_id") != CASE_ID
        or payload.get("repository") != REPOSITORY
        or payload.get("issue_number") != ISSUE_NUMBER
        or payload.get("issue_url") != ISSUE_URL
        or payload.get("action") not in ACTIVE_ACTIONS
    ):
        raise RouteError("route is outside the fixed canary")
    route_id = payload["route_id"]
    if not isinstance(route_id, str):
        raise RouteError("route ID is invalid")
    return route_id


def _task_body(route: Mapping[str, Any], phase: str, instructions: str) -> str:
    evidence = {
        key: route[key]
        for key in (
            "case_id",
            "repository",
            "issue_number",
            "issue_url",
            "route_id",
            "comment_id",
            "comment_url",
            "evidence_body_sha256",
            "planner_comment_id",
            "planner_body_sha256",
            "plan_version",
            "planned_base_sha",
        )
    }
    return (
        f"# Pip v2 {phase}\n\n"
        f"Authorization binding:\n```json\n{json.dumps(evidence, indent=2, sort_keys=True)}\n```\n\n"
        "Work only on marmot-protocol/mdk#1240 and a Pip-owned `pip/*` branch. "
        "Do not touch MLS/CGKA implementation, keys, credentials, trust anchors, "
        "membership/admin authorization semantics, or push payload context. "
        "Do not bump versions. Update the existing Unreleased changelog when code changes. "
        "Draft PR, exact-head green CI, independent review, and human merge are mandatory.\n\n"
        f"{instructions}\n"
    )


def builder_dag(route: Mapping[str, Any]) -> list[TaskSpec]:
    validate_route(route)
    if route.get("action") != "dispatch_builder":
        raise RouteError("builder DAG requires a builder route")
    return [
        TaskSpec(
            key="build",
            title="Pip v2 build mdk#1240",
            body=_task_body(
                route,
                "builder",
                "Use direct Cursor `cursor-grok-4.6-high` through the builder-grok workflow. "
                "Implement the approved planner comment exactly. Open a draft PR and do not "
                "complete until CI is green on its exact head.",
            ),
            assignee="cursor-fixer",
            parents=(),
            skills=("workflow-contract", "builder-grok"),
            max_runtime="4h",
        ),
        TaskSpec(
            key="review-general-1",
            title="Pip v2 correctness review mdk#1240 round 1",
            body=_task_body(
                route, "general review", "Review the parent PR exact head read-only."
            ),
            assignee="reviewer-general",
            parents=("build",),
            skills=("workflow-contract", "reviewer-general"),
            max_runtime="2h",
            model="gpt-5.6-sol",
            provider="openai-codex",
        ),
        TaskSpec(
            key="review-secperf-1",
            title="Pip v2 security/performance review mdk#1240 round 1",
            body=_task_body(
                route,
                "security review",
                "Use direct Cursor `kimi-k3-high` through the reviewer-secperf workflow. "
                "Review the parent PR exact head read-only.",
            ),
            assignee="cursor-reviewer",
            parents=("build",),
            skills=("workflow-contract", "reviewer-secperf"),
            max_runtime="2h",
        ),
        TaskSpec(
            key="remediate",
            title="Pip v2 address reviews mdk#1240",
            body=_task_body(
                route,
                "review remediation",
                "Read both review results. Use direct Cursor `cursor-grok-4.6-high` to address "
                "every blocking finding on the existing Pip branch. If no change is needed, "
                "record that fact. Require green CI on the resulting exact head.",
            ),
            assignee="cursor-fixer",
            parents=("build", "review-general-1", "review-secperf-1"),
            skills=("workflow-contract", "builder-grok"),
            max_runtime="4h",
        ),
        TaskSpec(
            key="review-general-2",
            title="Pip v2 correctness re-review mdk#1240",
            body=_task_body(
                route,
                "general re-review",
                "Independently re-review the current exact PR head and confirm every prior finding.",
            ),
            assignee="reviewer-general",
            parents=("remediate",),
            skills=("workflow-contract", "reviewer-general"),
            max_runtime="2h",
            model="gpt-5.6-sol",
            provider="openai-codex",
        ),
        TaskSpec(
            key="review-secperf-2",
            title="Pip v2 security/performance re-review mdk#1240",
            body=_task_body(
                route,
                "security re-review",
                "Use direct Cursor `kimi-k3-high`. Independently review the current exact PR "
                "head and confirm every prior finding.",
            ),
            assignee="cursor-reviewer",
            parents=("remediate",),
            skills=("workflow-contract", "reviewer-secperf"),
            max_runtime="2h",
        ),
        TaskSpec(
            key="final-review",
            title="Pip v2 final review mdk#1240",
            body=_task_body(
                route,
                "final review",
                "Verify both required reviews approve the same current head, CI is green on that "
                "head, and no blocking finding remains. HUMAN MERGE ONLY. Return durable final "
                "metadata to the deterministic gate; do not notify anyone and never merge.",
            ),
            assignee="final-reviewer",
            parents=("review-general-2", "review-secperf-2"),
            skills=("workflow-contract", "final-reviewer"),
            max_runtime="2h",
            model="gpt-5.6-sol",
            provider="openai-codex",
        ),
    ]


def _default_runner(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=False, capture_output=True, text=True)


def _created_task_info(
    completed: subprocess.CompletedProcess[str],
) -> tuple[str, str | None]:
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise RouteError(f"Kanban task creation failed: {detail}")
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise RouteError("Kanban returned malformed task JSON") from exc
    if isinstance(payload, dict):
        task_id = payload.get("id") or payload.get("task_id")
        if task_id is None and isinstance(payload.get("task"), dict):
            task_id = payload["task"].get("id")
        if isinstance(task_id, str) and task_id:
            task = (
                payload.get("task")
                if isinstance(payload.get("task"), dict)
                else payload
            )
            status = task.get("status") if isinstance(task, dict) else None
            return task_id, status if isinstance(status, str) else None
    raise RouteError("Kanban did not return a task ID")


def _created_task_id(completed: subprocess.CompletedProcess[str]) -> str:
    return _created_task_info(completed)[0]


def _gate_sentinel(
    board: str,
    route_id: str,
    runner: Callable[[list[str]], subprocess.CompletedProcess[str]],
) -> str:
    del runner
    try:
        return create_sticky_sentinel(board, route_id)
    except SentinelError as exc:
        raise RouteError(str(exc)) from exc


def _activation_gate_command(
    board: str, route_id: str, key: str, sentinel_id: str
) -> list[str]:
    return [
        "hermes",
        "kanban",
        "--board",
        board,
        "create",
        f"Pip v2 activation gate: {key}",
        "--body",
        json.dumps(
            {
                "activation_gate": key,
                "case_id": "mdk#1240",
                "route_id": route_id,
            },
            sort_keys=True,
        ),
        "--idempotency-key",
        f"pip-v2:{route_id}:gate:{key}",
        "--created-by",
        "pip-v2-router",
        "--parent",
        sentinel_id,
        "--max-retries",
        "1",
        "--initial-status",
        "blocked",
        "--json",
    ]


def _release_activation_gate(
    board: str,
    gate_id: str,
    gate_status: str | None,
    runner: Callable[[list[str]], subprocess.CompletedProcess[str]],
    before_activate: Callable[[], None] | None,
) -> bool:
    if gate_status not in (None, "blocked"):
        return False
    if before_activate is not None:
        before_activate()
    completed = runner(
        [
            "hermes",
            "kanban",
            "--board",
            board,
            "complete",
            gate_id,
            "--result",
            "live authorization validated; release exactly one worker",
        ]
    )
    if completed.returncode != 0:
        raise RouteError("Kanban activation-gate completion failed")
    return True


def route_once(
    route: Mapping[str, Any],
    *,
    runner: Callable[[list[str]], subprocess.CompletedProcess[str]] = _default_runner,
    board: str = "pip-mdk",
    before_activate: Callable[[], None] | None = None,
) -> dict[str, str]:
    route_id = validate_route(route)
    sentinel_id = _gate_sentinel(board, route_id, runner)
    if route.get("action") == "replan":
        body = _task_body(
            route,
            "replan",
            "Produce a fresh plan against the current default-branch head. Preserve the human's "
            f"exact narrowed scope: {route.get('narrowed_scope')!r}. Publish or update the "
            "attributable planner comment, then stop for a new explicit human decision. Do not "
            "create a branch or PR.",
        )
        gate_id, gate_status = _created_task_info(
            runner(_activation_gate_command(board, route_id, "replan", sentinel_id))
        )
        create = [
            "hermes",
            "kanban",
            "--board",
            board,
            "create",
            "Pip v2 replan mdk#1240",
            "--body",
            body,
            "--assignee",
            "planner",
            "--workspace",
            "scratch",
            "--parent",
            gate_id,
            "--idempotency-key",
            f"pip-v2:{route_id}:replan",
            "--created-by",
            "pip-v2-router",
            "--skill",
            "workflow-contract",
            "--skill",
            "planner",
            "--max-runtime",
            "30m",
            "--max-retries",
            "1",
            "--model",
            "gpt-5.6-sol",
            "--provider",
            "openai-codex",
            "--initial-status",
            "blocked",
            "--json",
        ]
        task_id, task_status = _created_task_info(runner(create))
        prefix = ["hermes", "kanban", "--board", board]
        if task_status not in (None, "blocked") or gate_status not in (
            None,
            "blocked",
        ):
            return {"replan": task_id}
        subscribed = runner(
            [
                *prefix,
                "notify-subscribe",
                task_id,
                "--platform",
                "telegram",
                "--chat-id",
                "483923125",
                "--chat-type",
                "dm",
                "--user-id",
                "483923125",
                "--notifier-profile",
                "default",
            ]
        )
        if subscribed.returncode != 0:
            raise RouteError("Kanban notification subscription failed")
        _release_activation_gate(board, gate_id, gate_status, runner, before_activate)
        return {"replan": task_id}
    if route.get("action") != "dispatch_builder":
        raise RouteError("route action is not executable")
    created: dict[str, str] = {}
    statuses: dict[str, str | None] = {}
    gate_statuses: dict[str, str | None] = {}
    for priority, spec in enumerate(builder_dag(route), start=100):
        gate_key = f"gate:{spec.key}"
        created[gate_key], gate_statuses[spec.key] = _created_task_info(
            runner(_activation_gate_command(board, route_id, spec.key, sentinel_id))
        )
        command = [
            "hermes",
            "kanban",
            "--board",
            board,
            "create",
            spec.title,
            "--body",
            spec.body,
            "--assignee",
            spec.assignee,
            "--workspace",
            "scratch",
            "--parent",
            created[gate_key],
            "--idempotency-key",
            f"pip-v2:{route_id}:{spec.key}",
            "--created-by",
            "pip-v2-router",
            "--max-runtime",
            spec.max_runtime,
            "--max-retries",
            "1",
            "--priority",
            str(priority),
        ]
        for skill in spec.skills:
            command.extend(["--skill", skill])
        for parent_key in spec.parents:
            command.extend(["--parent", created[parent_key]])
        if spec.model is not None:
            command.extend(["--model", spec.model, "--provider", str(spec.provider)])
        command.extend(["--initial-status", "blocked"])
        command.append("--json")
        created[spec.key], statuses[spec.key] = _created_task_info(runner(command))
    if statuses["build"] in (None, "blocked"):
        _release_activation_gate(
            board,
            created["gate:build"],
            gate_statuses["build"],
            runner,
            before_activate,
        )
    return created
