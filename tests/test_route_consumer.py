from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest

from pip_agent.decision_reconciler import (
    ExternalDecisionError,
    HumanDecision,
    PlannerEvidence,
)
from pip_agent.route_consumer import (
    RouteConsumerError,
    _archive_canary_tasks,
    _validate_live_route,
    consume_route,
    render_route_consumer_service,
)


def test_passive_route_does_not_touch_kanban() -> None:
    route = {
        "ok": True,
        "case_id": "mdk#1240",
        "action": "continue",
        "state": "PLANNING",
    }
    assert consume_route(route, board="pip-mdk") == {
        "status": "passive",
        "action": "continue",
    }


def test_router_unit_has_no_custom_ledger_or_worktree_mutation() -> None:
    unit = render_route_consumer_service("jeff", "jeff", Path("/home/jeff"))
    assert "--board pip-mdk" in unit
    assert "--skills-repository-commit-file /opt/pip-v2/SOURCE.COMMIT" in unit
    assert "ledger" not in unit
    assert "/code/worktrees" not in unit
    assert "ReadWritePaths=/home/jeff/.hermes" in unit
    assert "Environment=PATH=/home/jeff/.local/bin:/usr/local/bin:/usr/bin:/bin" in unit


def test_stop_archives_only_nonterminal_pip_v2_canary_tasks() -> None:
    commands: list[list[str]] = []
    terminated: list[int] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "list" in command:
            output = json.dumps(
                [
                    {
                        "id": "t-active",
                        "created_by": "pip-v2-router",
                        "body": 'Authorization: {"case_id": "mdk#1240"}',
                        "status": "running",
                    },
                    {
                        "id": "t-v1",
                        "created_by": "pip-issue-scanner",
                        "body": 'Authorization: {"case_id": "mdk#1240"}',
                        "status": "ready",
                    },
                    {
                        "id": "t-staged",
                        "created_by": "pip-v2-router",
                        "body": 'Authorization: ***"case_id": "mdk#1240"',
                        "status": "blocked",
                    },
                    {
                        "id": "t-done-gate",
                        "created_by": "pip-v2-router",
                        "body": '{"activation_gate": "build", "case_id": "mdk#1240", "route_id": "old"}',
                        "status": "done",
                    },
                    {
                        "id": "t-final-disposition",
                        "created_by": "pip-v2-router",
                        "body": '{"case_id": "mdk#1240", "outcome": "BLOCKED", "route_id": "old"}\nBlocked disposition',
                        "status": "blocked",
                    },
                ]
            )
        elif "show" in command:
            output = json.dumps({"runs": [{"worker_pid": 1234}]})
        else:
            output = ""
        return subprocess.CompletedProcess(command, 0, output, "")

    result = consume_route(
        {"ok": False, "case_id": "mdk#1240", "action": "stop"},
        board="pip-mdk",
        runner=runner,
        terminator=terminated.append,
    )

    assert result["archived_tasks"] == [
        "t-active",
        "t-staged",
        "t-done-gate",
        "t-final-disposition",
    ]
    assert terminated == [1234]
    assert commands[2][commands[2].index("archive") + 1] == "t-active"
    assert "t-staged" in commands[2]
    assert "t-done-gate" in commands[2]
    assert "t-final-disposition" in commands[2]
    assert "t-v1" not in commands[2]


def test_dispatch_upgrade_archives_only_superseded_same_route_dag() -> None:
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "list" in command:
            output = json.dumps(
                [
                    {
                        "id": "t-v3",
                        "created_by": "pip-v2-router",
                        "body": '{"case_id": "mdk#1240", "route_id": "route-a", "dag_revision": 3}',
                        "status": "done",
                    },
                    {
                        "id": "t-v4",
                        "created_by": "pip-v2-router",
                        "body": '{"case_id": "mdk#1240", "route_id": "route-a", "dag_revision": 4}',
                        "status": "blocked",
                    },
                ]
            )
        else:
            output = ""
        return subprocess.CompletedProcess(command, 0, output, "")

    archived = _archive_canary_tasks(
        "pip-mdk",
        runner,
        preserve_route_id="route-a",
        preserve_dag_revision=4,
    )

    assert archived == ["t-v3"]
    assert commands[-1][-1] == "t-v3"


def test_live_route_revalidation_allows_base_change_before_dispatch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    planned = "a" * 40
    planner = PlannerEvidence(7, "https://planner", "b" * 64, 2, planned, "now")
    decision = HumanDecision(
        "approve", "erskingardner", 8, "https://decision", "c" * 64, None
    )
    route = {
        "action": "dispatch_builder",
        "decision": "approve",
        "comment_id": 8,
        "comment_url": "https://decision",
        "evidence_body_sha256": "c" * 64,
        "planner_comment_id": 7,
        "planner_comment_url": "https://planner",
        "planner_body_sha256": "b" * 64,
        "plan_version": 2,
        "planned_base_sha": planned,
        "narrowed_scope": None,
    }
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.fetch_canary_issue_authorization", lambda: None
    )
    monkeypatch.setattr("pip_agent.decision_reconciler.fetch_canary_comments", list)
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_planner_evidence", lambda _: planner
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_human_decision", lambda _: decision
    )
    _validate_live_route(route)


def test_live_route_revalidation_accepts_current_automatic_proceed_plan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    planner = PlannerEvidence(
        7,
        "https://planner",
        "b" * 64,
        2,
        "a" * 40,
        "now",
        outcome="PROCEED",
    )
    route = {
        "action": "dispatch_builder",
        "decision": "automatic",
        "comment_id": 7,
        "comment_url": "https://planner",
        "evidence_body_sha256": "b" * 64,
        "planner_comment_id": 7,
        "planner_comment_url": "https://planner",
        "planner_body_sha256": "b" * 64,
        "plan_version": 2,
        "planned_base_sha": "a" * 40,
        "planner_outcome": "PROCEED",
        "narrowed_scope": None,
    }
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.fetch_canary_issue_authorization", lambda: None
    )
    monkeypatch.setattr("pip_agent.decision_reconciler.fetch_canary_comments", list)
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_planner_evidence", lambda _: planner
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_human_decision", lambda _: None
    )

    _validate_live_route(route)


def test_live_automatic_route_stays_bound_to_original_planner_comment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    planner = PlannerEvidence(
        7,
        "https://planner",
        "b" * 64,
        2,
        "a" * 40,
        "now",
        outcome="PROCEED",
    )
    route = {
        "action": "dispatch_builder",
        "decision": "automatic",
        "comment_id": 7,
        "comment_url": "https://planner",
        "evidence_body_sha256": "b" * 64,
        "planner_comment_id": 7,
        "planner_comment_url": "https://planner",
        "planner_body_sha256": "b" * 64,
        "plan_version": 2,
        "planned_base_sha": "a" * 40,
        "planner_outcome": "PROCEED",
        "narrowed_scope": None,
    }
    comments = [
        {"id": 7, "user": {"login": "agent-p1p"}},
        {"id": 9, "user": {"login": "agent-p1p"}},
    ]
    seen: list[list[int]] = []

    def parse_bound(received):
        seen.append([comment["id"] for comment in received])
        return planner

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.fetch_canary_issue_authorization", lambda: None
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.fetch_canary_comments", lambda: comments
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_planner_evidence", parse_bound
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_human_decision",
        lambda received: seen.append([comment["id"] for comment in received]),
    )

    _validate_live_route(route)

    assert seen == [[7], [7]]


def test_live_automatic_route_rejects_later_human_decision(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    planner = PlannerEvidence(
        7,
        "https://planner",
        "b" * 64,
        2,
        "a" * 40,
        "now",
        outcome="PROCEED",
    )
    decision = HumanDecision(
        "reject", "erskingardner", 8, "https://decision", "c" * 64, None
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.fetch_canary_issue_authorization", lambda: None
    )
    monkeypatch.setattr("pip_agent.decision_reconciler.fetch_canary_comments", list)
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_planner_evidence", lambda _: planner
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.parse_human_decision", lambda _: decision
    )

    with pytest.raises(RouteConsumerError, match="supersedes"):
        _validate_live_route({"action": "dispatch_builder", "decision": "automatic"})


def _active_task_runner(
    commands: list[list[str]], *, worker_pid: int
) -> Callable[[list[str]], subprocess.CompletedProcess[str]]:
    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "list" in command:
            output = json.dumps(
                [
                    {
                        "id": "t-active",
                        "created_by": "pip-v2-router",
                        "body": 'Authorization: ***"case_id": "mdk#1240"}',
                        "status": "running",
                    }
                ]
            )
        elif "show" in command:
            output = json.dumps({"runs": [{"worker_pid": worker_pid}]})
        else:
            output = ""
        return subprocess.CompletedProcess(command, 0, output, "")

    return runner


def test_replan_quiesces_superseded_workers_before_planner_activation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    commands: list[list[str]] = []
    terminated: list[int] = []
    routed_after_quiesce: list[bool] = []
    runner = _active_task_runner(commands, worker_pid=4321)

    def fake_route_once(route, *, board, runner, before_activate):
        routed_after_quiesce.append(terminated == [4321])
        before_activate()
        return {"replan": "t-replan"}

    monkeypatch.setattr("pip_agent.route_consumer.route_once", fake_route_once)
    result = consume_route(
        {"ok": True, "case_id": "mdk#1240", "action": "replan"},
        board="pip-mdk",
        runner=runner,
        terminator=terminated.append,
        live_validator=lambda route: None,
    )

    assert result["tasks"] == {"replan": "t-replan"}
    assert routed_after_quiesce == [True]
    assert terminated == [4321]


def test_live_revocation_during_staging_stops_existing_worker_and_never_gates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    commands: list[list[str]] = []
    terminated: list[int] = []
    validations = 0
    gate_calls = 0
    runner = _active_task_runner(commands, worker_pid=9876)

    def live_validator(route) -> None:
        nonlocal validations
        validations += 1
        raise RouteConsumerError("authorization changed during staging")

    def fake_route_once(route, *, board, runner, before_activate):
        before_activate()
        raise AssertionError("activation must not follow revoked authorization")

    def fake_gate(*args, **kwargs):
        nonlocal gate_calls
        gate_calls += 1

    monkeypatch.setattr("pip_agent.route_consumer.route_once", fake_route_once)

    with pytest.raises(RouteConsumerError, match="changed during staging"):
        consume_route(
            {"ok": True, "case_id": "mdk#1240", "action": "dispatch_builder"},
            board="pip-mdk",
            runner=runner,
            terminator=terminated.append,
            live_validator=live_validator,
            gate_advancer=fake_gate,
        )

    assert validations == 1
    assert terminated == [9876]
    assert gate_calls == 0


def test_transient_live_lookup_failure_preserves_existing_worker_and_never_gates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    commands: list[list[str]] = []
    terminated: list[int] = []
    gate_calls = 0
    runner = _active_task_runner(commands, worker_pid=9876)

    def live_validator(route) -> None:
        raise ExternalDecisionError("GitHub decision lookup failed")

    def fake_route_once(route, *, board, runner, before_activate):
        before_activate()
        raise AssertionError("activation must not follow a failed lookup")

    def fake_gate(*args, **kwargs):
        nonlocal gate_calls
        gate_calls += 1

    monkeypatch.setattr("pip_agent.route_consumer.route_once", fake_route_once)

    with pytest.raises(ExternalDecisionError, match="lookup failed"):
        consume_route(
            {"ok": True, "case_id": "mdk#1240", "action": "dispatch_builder"},
            board="pip-mdk",
            runner=runner,
            terminator=terminated.append,
            live_validator=live_validator,
            gate_advancer=fake_gate,
        )

    assert terminated == []
    assert gate_calls == 0


def test_unchanged_materialized_route_does_not_revalidate_without_activation(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    validations = 0

    def live_validator(route) -> None:
        nonlocal validations
        validations += 1

    monkeypatch.setattr(
        "pip_agent.route_consumer.route_once",
        lambda *args, **kwargs: {"build": "t-build"},
    )
    result = consume_route(
        {"ok": True, "case_id": "mdk#1240", "action": "dispatch_builder"},
        board="pip-mdk",
        runner=lambda command: subprocess.CompletedProcess(command, 0, "", ""),
        live_validator=live_validator,
        gate_advancer=lambda *args, **kwargs: {"status": "waiting"},
    )

    assert result["gate"] == {"status": "waiting"}
    assert validations == 0
