from __future__ import annotations

import copy
import hashlib
import json
import subprocess

import pytest

from pip_agent import kanban_router
from pip_agent.kanban_router import (
    RouteError,
    builder_dag,
    canonical_route_id,
    route_once,
    validate_route,
)


@pytest.fixture(autouse=True)
def _atomic_sentinel(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        kanban_router,
        "create_sticky_sentinel",
        lambda board, route_id: "t-gate-sentinel",
    )


def _active_route(*, action: str = "dispatch_builder") -> dict[str, object]:
    body = "Pip: approve exact scope"
    route: dict[str, object] = {
        "ok": True,
        "case_id": "mdk#1240",
        "repository": "marmot-protocol/mdk",
        "issue_number": 1240,
        "issue_url": "https://github.com/marmot-protocol/mdk/issues/1240",
        "action": action,
        "state": "BUILDING" if action == "dispatch_builder" else "PLANNING",
        "decision": "approve" if action == "dispatch_builder" else "narrow",
        "comment_id": 5304234569,
        "comment_url": "https://github.com/marmot-protocol/mdk/issues/1240#issuecomment-5304234569",
        "evidence_body_sha256": hashlib.sha256(body.encode()).hexdigest(),
        "planner_comment_id": 5304234568,
        "planner_body_sha256": "a" * 64,
        "plan_version": 2,
        "planned_base_sha": "b" * 40,
        "narrowed_scope": None,
    }
    if action == "replan":
        route["narrowed_scope"] = "projection only"
    route["route_id"] = canonical_route_id(route)
    return route


def test_route_id_binds_every_executable_field() -> None:
    route = _active_route()
    assert validate_route(route) == route["route_id"]

    for key, replacement in (
        ("action", "replan"),
        ("state", "PLANNING"),
        ("comment_url", "https://example.invalid/comment"),
        ("planned_base_sha", "c" * 40),
        ("plan_version", 3),
    ):
        mutated = copy.deepcopy(route)
        mutated[key] = replacement
        with pytest.raises(RouteError, match="route ID"):
            validate_route(mutated)


def test_builder_route_creates_v1_style_review_dag() -> None:
    route = _active_route()
    specs = builder_dag(route)

    assert [spec.key for spec in specs] == [
        "build",
        "review-general-1",
        "review-secperf-1",
        "remediate",
        "review-general-2",
        "review-secperf-2",
        "final-review",
    ]
    by_key = {spec.key: spec for spec in specs}
    assert by_key["build"].assignee == "cursor-fixer"
    assert by_key["build"].skills == ("workflow-contract", "builder-grok")
    assert by_key["review-secperf-1"].skills == (
        "workflow-contract",
        "reviewer-secperf",
    )
    assert by_key["review-general-1"].parents == ("build",)
    assert by_key["review-secperf-1"].assignee == "cursor-reviewer"
    assert by_key["remediate"].parents == (
        "build",
        "review-general-1",
        "review-secperf-1",
    )
    assert by_key["review-general-2"].parents == ("remediate",)
    assert by_key["review-secperf-2"].parents == ("remediate",)
    assert by_key["final-review"].parents == (
        "review-general-2",
        "review-secperf-2",
    )
    assert "HUMAN MERGE ONLY" in by_key["final-review"].body
    assert "do not notify anyone" in by_key["final-review"].body

    bindings = {}
    for key, spec in by_key.items():
        encoded = spec.body.split("Authorization binding:\n```json\n", 1)[1].split(
            "\n```", 1
        )[0]
        bindings[key] = json.loads(encoded)
    assert (
        bindings["build"]
        | {
            "execution_mode": "direct-cursor",
            "execution_model": "composer-2.5",
            "execution_provider": "cursor",
        }
        == bindings["build"]
    )
    assert bindings["remediate"]["execution_model"] == "composer-2.5"
    assert bindings["review-secperf-1"]["execution_model"] == (
        "claude-opus-4-8-thinking-high"
    )
    assert bindings["review-secperf-2"]["execution_model"] == (
        "claude-opus-4-8-thinking-high"
    )
    assert bindings["review-general-1"]["execution_mode"] == "hermes"
    assert bindings["final-review"]["execution_provider"] == "openai-codex"


def test_builder_dag_binds_exact_installed_skills_commit() -> None:
    specs = kanban_router.builder_dag(
        _active_route(), skills_repository_commit="a" * 40
    )
    for spec in specs:
        encoded = spec.body.split("Authorization binding:\n```json\n", 1)[1].split(
            "\n```", 1
        )[0]
        assert json.loads(encoded)["skills_repository_commit"] == "a" * 40


def test_dag_upgrade_revisions_builder_after_historical_red_ci() -> None:
    route_id = str(_active_route()["route_id"])
    assert kanban_router._task_idempotency_key(route_id, "build") == (
        f"pip-v2:{route_id}:dag-v3:build"
    )
    assert kanban_router._task_idempotency_key(route_id, "review-general-1") == (
        f"pip-v2:{route_id}:dag-v3:review-general-1"
    )
    assert kanban_router._gate_idempotency_key(route_id, "build") == (
        f"pip-v2:{route_id}:gate:dag-v3:build"
    )
    assert kanban_router._gate_idempotency_key(route_id, "review-general-1") == (
        f"pip-v2:{route_id}:gate:dag-v3:review-general-1"
    )


def test_builder_dag_uses_kanban_idempotency_and_real_parent_ids() -> None:
    commands: list[list[str]] = []
    statuses: dict[str, str] = {}

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "complete" in command:
            statuses[command[command.index("complete") + 1]] = "done"
            return subprocess.CompletedProcess(command, 0, "", "")
        if "create" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        idem = command[command.index("--idempotency-key") + 1]
        key = idem.rsplit(":", 1)[-1]
        task_id = f"t-gate-{key}" if ":gate:" in idem else f"t-{key}"
        statuses.setdefault(task_id, "triage" if key == "gate-sentinel" else "blocked")
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps({"id": task_id, "status": statuses[task_id]}),
            "",
        )

    route = _active_route()
    first = route_once(route, runner=runner)
    second = route_once(route, runner=runner)

    assert first == second
    assert first["build"] == "t-build"
    final_command = next(
        command
        for command in commands
        if "create" in command
        and command[command.index("--idempotency-key") + 1].endswith(":final-review")
        and ":gate:" not in command[command.index("--idempotency-key") + 1]
    )
    assert final_command.count("--parent") == 3
    assert "t-gate-final-review" in final_command
    assert "t-review-general-2" in final_command
    assert "t-review-secperf-2" in final_command
    create_commands = [command for command in commands if "create" in command]
    assert [
        command[command.index("--idempotency-key") + 1]
        for command in create_commands[:14]
    ] == [
        command[command.index("--idempotency-key") + 1]
        for command in create_commands[14:]
    ]
    gate_commands = [
        command
        for command in create_commands
        if ":gate:" in command[command.index("--idempotency-key") + 1]
    ]
    sentinel_commands = [
        command
        for command in create_commands
        if command[command.index("--idempotency-key") + 1].endswith(":gate-sentinel")
    ]
    worker_commands = [
        command
        for command in create_commands
        if command not in gate_commands and command not in sentinel_commands
    ]
    assert all("--initial-status" in command for command in gate_commands)
    assert all("--parent" in command for command in gate_commands)
    assert all("t-gate-sentinel" in command for command in gate_commands)
    assert all("--initial-status" in command for command in worker_commands)
    assert all(
        command[command.index("--max-retries") + 1] == "1"
        for command in create_commands
    )
    complete_commands = [command for command in commands if "complete" in command]
    assert complete_commands == [
        [
            "hermes",
            "kanban",
            "--board",
            "pip-mdk",
            "complete",
            "t-gate-build",
            "--result",
            "live authorization validated; release exactly one worker",
        ]
    ]
    assert len(commands) == 29


def test_builder_is_revalidated_after_complete_staging_before_activation() -> None:
    commands: list[list[str]] = []
    callback_observations: list[tuple[int, int]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "create" in command:
            key = command[command.index("--idempotency-key") + 1].rsplit(":", 1)[-1]
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "id": f"t-{key}",
                        "status": "triage" if key == "gate-sentinel" else "blocked",
                    }
                ),
                "",
            )
        return subprocess.CompletedProcess(command, 0, "", "")

    def before_activate() -> None:
        callback_observations.append(
            (
                sum("create" in command for command in commands),
                sum("complete" in command for command in commands),
            )
        )

    route_once(_active_route(), runner=runner, before_activate=before_activate)

    assert callback_observations == [(14, 0)]
    assert "complete" in commands[-1]


def test_replan_task_is_staged_subscribed_then_gate_released() -> None:
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        output = ""
        if "create" in command:
            idem = command[command.index("--idempotency-key") + 1]
            output = json.dumps(
                {
                    "id": "t-sentinel"
                    if idem.endswith(":gate-sentinel")
                    else "t-replan",
                    "status": "triage"
                    if idem.endswith(":gate-sentinel")
                    else "blocked",
                }
            )
        return subprocess.CompletedProcess(command, 0, output, "")

    result = route_once(_active_route(action="replan"), runner=runner)

    assert result == {"replan": "t-replan"}
    assert "--initial-status" in commands[0]
    assert "notify-subscribe" in commands[2]
    assert "complete" in commands[3]
    assert commands[3][commands[3].index("complete") + 1] == "t-replan"


def test_partial_dag_failure_never_activates_builder() -> None:
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if len(commands) == 1:
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps({"id": "t-sentinel", "status": "triage"}),
                "",
            )
        return subprocess.CompletedProcess(command, 1, "", "injected create failure")

    with pytest.raises(RouteError):
        route_once(_active_route(), runner=runner)

    assert commands[0][commands[0].index("--idempotency-key") + 1].endswith(
        ":gate:dag-v3:build"
    )
    assert not any("complete" in command for command in commands)


def test_reprocessing_replan_does_not_reactivate_an_existing_task() -> None:
    commands: list[list[str]] = []
    statuses = {
        "t-sentinel": "triage",
        "t-gate-replan": "blocked",
        "t-replan": "blocked",
    }

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "create" in command:
            idem = command[command.index("--idempotency-key") + 1]
            if idem.endswith(":gate-sentinel"):
                task_id = "t-sentinel"
            else:
                task_id = "t-gate-replan" if ":gate:" in idem else "t-replan"
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps({"id": task_id, "status": statuses[task_id]}),
                "",
            )
        if "complete" in command:
            statuses[command[command.index("complete") + 1]] = "done"
        return subprocess.CompletedProcess(command, 0, "", "")

    route = _active_route(action="replan")
    assert route_once(route, runner=runner) == {"replan": "t-replan"}
    assert route_once(route, runner=runner) == {"replan": "t-replan"}
    assert sum("complete" in command for command in commands) == 1
    assert len(commands) == 6
    assert "create" in commands[-1]
