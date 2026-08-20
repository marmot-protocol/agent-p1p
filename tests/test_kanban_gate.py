from __future__ import annotations

import copy
import json
import subprocess
from typing import Any

import pytest

from pip_agent import kanban_gate
from pip_agent.kanban_gate import GateError
from pip_agent.kanban_gate import advance_gates as _advance_gates
from pip_agent.offline_fixture import (
    DECISION_BODY_SHA,
    PLANNED_BASE_SHA,
    PLANNER_BODY_SHA,
    ROUTE_ID,
    _builder,
    _final,
    _review,
)


def advance_gates(*args: Any, **kwargs: Any) -> dict[str, Any]:
    kwargs.setdefault("implementation_base_validator", lambda _base, _head: None)
    runner = kwargs.get("runner")
    if runner is not None:

        def runner_with_durable_model(
            command: list[str],
        ) -> subprocess.CompletedProcess[str]:
            completed = runner(command)
            if "show" not in command or completed.returncode != 0:
                return completed
            payload = json.loads(completed.stdout)
            runs = payload.get("runs", [])
            if runs:
                raw_metadata = runs[-1].get("metadata")
                metadata = (
                    json.loads(raw_metadata)
                    if isinstance(raw_metadata, str)
                    else raw_metadata
                )
                if isinstance(metadata, dict):
                    metadata = copy.deepcopy(metadata)
                    metadata.setdefault("artifacts", ["/tmp/result.json"])
                    metadata.setdefault("worker_session_id", "session-trusted")
                    payload["runs"][-1]["metadata"] = metadata
                    role = metadata.get("role")
                    role = role if isinstance(role, str) else ""
                    profiles = {
                        "builder-grok": "cursor-fixer",
                        "reviewer-general": "reviewer-general",
                        "reviewer-secperf": "cursor-reviewer",
                        "final-reviewer": "final-reviewer",
                    }
                    role_skills = {
                        "builder-grok": ["workflow-contract", "builder-grok"],
                        "reviewer-general": ["workflow-contract", "reviewer-general"],
                        "reviewer-secperf": ["workflow-contract", "reviewer-secperf"],
                        "final-reviewer": ["workflow-contract", "final-reviewer"],
                    }
                    payload["runs"][-1].setdefault("outcome", "completed")
                    payload["runs"][-1].setdefault("profile", profiles.get(role))
                    task = payload["task"]
                    task.setdefault("assignee", profiles.get(role))
                    task.setdefault("skills", role_skills.get(role))
                    actual = metadata.get("actual_model")
                    if isinstance(actual, str) and actual:
                        if "/" in actual:
                            provider, model = actual.split("/", 1)
                        else:
                            provider, model = "openai-codex", actual
                        mode = "direct-cursor" if provider == "cursor" else "hermes"
                        binding = {
                            "execution_mode": mode,
                            "execution_model": model,
                            "execution_provider": provider,
                        }
                        task.setdefault(
                            "body",
                            "Authorization binding:\n```json\n"
                            + json.dumps(binding, sort_keys=True)
                            + "\n```",
                        )
                        if mode == "hermes":
                            task.setdefault("model_override", model)
                            task.setdefault("provider_override", provider)
                    return subprocess.CompletedProcess(
                        command,
                        completed.returncode,
                        json.dumps(payload),
                        completed.stderr,
                    )
            return completed

        kwargs["runner"] = runner_with_durable_model
    return _advance_gates(*args, **kwargs)


def _route() -> dict[str, object]:
    return {
        "case_id": "mdk#900001",
        "route_id": ROUTE_ID,
        "comment_id": 2,
        "evidence_body_sha256": DECISION_BODY_SHA,
        "planner_comment_id": 1,
        "planner_body_sha256": PLANNER_BODY_SHA,
        "planned_base_sha": PLANNED_BASE_SHA,
        "plan_version": 1,
    }


def _results() -> dict[str, dict[str, object]]:
    build = _builder()
    build["task_id"] = "t-build"
    remediation = copy.deepcopy(build)
    remediation["task_id"] = "t-remediate"
    remediation["build_round"] = 2
    general1 = _review("reviewer-general", "openai-codex/gpt-5.6-sol")
    general1["task_id"] = "t-general-1"
    secperf1 = _review("reviewer-secperf", "cursor/claude-opus-4-8-thinking-high")
    secperf1["task_id"] = "t-secperf-1"
    general2 = copy.deepcopy(general1)
    general2["task_id"] = "t-general-2"
    general2["review_round"] = 2
    secperf2 = copy.deepcopy(secperf1)
    secperf2["task_id"] = "t-secperf-2"
    secperf2["review_round"] = 2
    final = _final()
    final["task_id"] = "t-final"
    return {
        "build": build,
        "review-general-1": general1,
        "review-secperf-1": secperf1,
        "remediate": remediation,
        "review-general-2": general2,
        "review-secperf-2": secperf2,
        "final-review": final,
    }


def _live_evidence(_pr: int, head: str) -> dict[str, object]:
    return {
        "ci": {
            "head_sha": head,
            "completed": True,
            "hollow": False,
            "rate_limited": False,
            "required_checks_green": True,
        },
        "open_blocking_threads": 0,
        "pip_owned": True,
        "mergeable": True,
        "issue_authorization_valid": True,
    }


def _final_snapshot(_pr: int, _head: str) -> None:
    return None


def _task_ids() -> dict[str, str]:
    workers = {
        "build": "t-build",
        "review-general-1": "t-general-1",
        "review-secperf-1": "t-secperf-1",
        "remediate": "t-remediate",
        "review-general-2": "t-general-2",
        "review-secperf-2": "t-secperf-2",
        "final-review": "t-final",
    }
    return {
        **workers,
        **{f"gate:{key}": f"t-gate-{key}" for key in workers},
    }


def _gate_response(
    command: list[str], key: str, task_id: str
) -> subprocess.CompletedProcess[str] | None:
    if not key.startswith("gate:"):
        return None
    status = "blocked" if key == "gate:final-review" else "done"
    payload = {
        "task": {"id": task_id, "status": status},
        "runs": [],
        "events": [{"kind": "created"}],
    }
    return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")


def test_semantic_gate_releases_final_only_after_valid_same_head_results() -> None:
    results = _results()
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "show" in command:
            task_id = command[command.index("show") + 1]
            key = next(key for key, value in _task_ids().items() if value == task_id)
            gate_response = _gate_response(command, key, task_id)
            if gate_response is not None:
                return gate_response
            if key == "final-review":
                payload = {
                    "task": {"id": task_id, "status": "blocked"},
                    "runs": [],
                    "events": [{"kind": "created"}],
                }
            else:
                payload = {
                    "task": {"id": task_id, "status": "done"},
                    "runs": [{"metadata": json.dumps(results[key])}],
                    "events": [{"kind": "created"}],
                }
            return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")
        return subprocess.CompletedProcess(command, 0, "", "")

    result = advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)

    assert result["advanced"] == ["final-review"]
    assert not any("notify-subscribe" in command for command in commands)
    assert any("complete" in command for command in commands)


def test_semantic_gate_rejects_mismatched_second_review_head() -> None:
    results = _results()
    results["review-secperf-2"]["reviewed_head_sha"] = "d" * 40

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {
                "id": task_id,
                "status": "blocked" if key == "final-review" else "done",
            },
            "runs": [] if key == "final-review" else [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    with pytest.raises(GateError, match="different head"):
        advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)


def test_expected_model_policy_accepts_current_cursor_lanes() -> None:
    kanban_gate._assert_expected_model(
        {
            "requested_model": "cursor/composer-2.5",
            "actual_model": "cursor/composer-2.5",
        },
        "builder-grok",
    )
    kanban_gate._assert_expected_model(
        {
            "requested_model": "cursor/claude-opus-4-8-thinking-high",
            "actual_model": "cursor/claude-opus-4-8-thinking-high",
        },
        "reviewer-secperf",
    )


def test_semantic_gate_rejects_self_consistent_but_unauthorized_model() -> None:
    results = _results()
    results["build"]["requested_model"] = "cursor/auto"
    results["build"]["actual_model"] = "cursor/auto"

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {
                "id": task_id,
                "status": "blocked" if key == "final-review" else "done",
            },
            "runs": [] if key == "final-review" else [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    with pytest.raises(GateError, match="unauthorized model"):
        advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)


def _trust_show_envelope(show: dict[str, object], result: dict[str, object]) -> None:
    profiles = {
        "builder-grok": "cursor-fixer",
        "reviewer-general": "reviewer-general",
        "reviewer-secperf": "cursor-reviewer",
        "final-reviewer": "final-reviewer",
    }
    role = str(result["role"])
    task = show["task"]
    runs = show["runs"]
    assert isinstance(task, dict) and isinstance(runs, list) and isinstance(runs[-1], dict)
    task["assignee"] = profiles[role]
    task["skills"] = ["workflow-contract", role]
    result.setdefault("artifacts", [f"/tmp/{role}-result.json"])
    result.setdefault("worker_session_id", f"session-{role}")
    runs[-1]["profile"] = profiles[role]
    runs[-1]["outcome"] = "completed"


def test_builder_result_rejects_historically_red_pr_1515() -> None:
    result = _results()["build"]
    result["pr_number"] = 1515
    show: dict[str, object] = {
        "task": {"id": "t-build", "status": "done", "model": "cursor/composer-2.5"},
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    with pytest.raises(GateError, match="historically red"):
        kanban_gate._builder_result(show, _route(), expected_round=1)


def test_metadata_accepts_trusted_hermes_completion_envelope() -> None:
    result = _results()["build"]
    expected = copy.deepcopy(result)
    result["artifacts"] = ["/tmp/builder-result.json"]
    result["worker_session_id"] = "session-trusted"
    show = {
        "task": {
            "id": "t-build",
            "status": "done",
            "model": "cursor/composer-2.5",
        },
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    assert kanban_gate._metadata(show, "builder-result") == expected


def test_metadata_accepts_native_task_model_override_fields() -> None:
    result = _results()["review-general-1"]
    show = {
        "task": {
            "id": "t-general-1",
            "status": "done",
            "model_override": "gpt-5.6-sol",
            "provider_override": "openai-codex",
        },
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    assert kanban_gate._metadata(show, "review-result") == result


def test_metadata_accepts_direct_cursor_model_bound_in_task_body() -> None:
    result = _results()["build"]
    result["requested_model"] = "cursor/composer-2.5"
    result["actual_model"] = "cursor/composer-2.5"
    binding = {
        "execution_mode": "direct-cursor",
        "execution_model": "composer-2.5",
        "execution_provider": "cursor",
    }
    show = {
        "task": {
            "id": "t-build",
            "status": "done",
            "body": "Authorization binding:\n```json\n"
            + json.dumps(binding, sort_keys=True)
            + "\n```",
        },
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    assert kanban_gate._metadata(show, "builder-result") == result


def test_metadata_rejects_skills_commit_conflicting_with_task_binding() -> None:
    result = _results()["build"]
    binding = {
        "execution_mode": "direct-cursor",
        "execution_model": "composer-2.5",
        "execution_provider": "cursor",
        "skills_repository_commit": "c" * 40,
    }
    show = {
        "task": {
            "id": "t-build",
            "status": "done",
            "body": "Authorization binding:\n```json\n"
            + json.dumps(binding, sort_keys=True)
            + "\n```",
        },
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    with pytest.raises(GateError, match="skills repository commit"):
        kanban_gate._metadata(show, "builder-result")


def test_metadata_rejects_worker_profile_conflicting_with_result_role() -> None:
    result = _results()["build"]
    result["artifacts"] = ["/tmp/result.json"]
    result["worker_session_id"] = "session-trusted"
    show = {
        "task": {
            "id": "t-build",
            "status": "done",
            "body": "Authorization binding:\n```json\n"
            + json.dumps(
                {
                    "execution_mode": "direct-cursor",
                    "execution_model": "composer-2.5",
                    "execution_provider": "cursor",
                },
                sort_keys=True,
            )
            + "\n```",
        },
        "runs": [{"metadata": result, "profile": "reviewer-general"}],
    }
    _trust_show_envelope(show, result)
    show_runs = show["runs"]
    assert isinstance(show_runs, list) and isinstance(show_runs[-1], dict)
    show_runs[-1]["profile"] = "reviewer-general"

    with pytest.raises(GateError, match="worker profile conflicts"):
        kanban_gate._metadata(show, "builder-result")


def test_metadata_rejects_non_completed_run_and_archived_task() -> None:
    result = _results()["build"]
    show: dict[str, object] = {
        "task": {
            "id": "t-build",
            "status": "archived",
            "model": "cursor/composer-2.5",
        },
        "runs": [{"metadata": result, "outcome": "crashed"}],
    }
    _trust_show_envelope(show, result)
    task = show["task"]
    runs = show["runs"]
    assert isinstance(task, dict) and isinstance(runs, list) and isinstance(runs[-1], dict)
    task["status"] = "archived"
    runs[-1]["outcome"] = "crashed"

    with pytest.raises(GateError, match="not durably completed"):
        kanban_gate._metadata(show, "builder-result")


def test_metadata_rejects_hermes_override_conflicting_with_task_binding() -> None:
    result = _results()["review-general-1"]
    binding = {
        "execution_mode": "hermes",
        "execution_model": "gpt-5.6-sol",
        "execution_provider": "openai-codex",
    }
    show = {
        "task": {
            "id": "t-general-1",
            "status": "done",
            "body": "Authorization binding:\n```json\n"
            + json.dumps(binding, sort_keys=True)
            + "\n```",
            "model_override": "gpt-5.6-sol",
            "provider_override": "other-provider",
        },
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    with pytest.raises(GateError, match="execution binding"):
        kanban_gate._metadata(show, "review-result")


def test_metadata_rejects_worker_model_conflicting_with_durable_run() -> None:
    result = _results()["build"]
    show = {
        "task": {"id": "t-build", "status": "done", "model": "cursor/auto"},
        "runs": [
            {
                "provider": "untrusted",
                "model": "cursor/auto",
                "metadata": result,
            }
        ],
    }
    _trust_show_envelope(show, result)

    with pytest.raises(GateError, match="durable task metadata"):
        kanban_gate._metadata(show, "builder-result")


def test_metadata_rejects_missing_durable_model_metadata() -> None:
    result = _results()["build"]
    show = {
        "task": {"id": "t-build", "status": "done"},
        "runs": [{"metadata": result}],
    }
    _trust_show_envelope(show, result)

    with pytest.raises(GateError, match="model metadata is missing"):
        kanban_gate._metadata(show, "builder-result")


def test_semantic_gate_rejects_unresolved_first_round_finding() -> None:
    results = _results()
    results["review-general-1"]["outcome"] = "REQUEST_CHANGES"
    results["review-general-1"]["blocking_findings"] = [
        {
            "id": "GENERAL-R1-001",
            "summary": "fixture blocker",
            "defect": "fixture defect",
            "consequence": "fixture consequence",
            "corrective_direction": "fix it",
            "required_evidence": ["same-head confirmation"],
            "status": "OPEN",
        }
    ]

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    with pytest.raises(
        GateError, match="does not resolve the exact round-1 finding set"
    ):
        advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)


def test_semantic_gate_rejects_result_from_another_decision_route() -> None:
    results = _results()
    results["build"]["route_id"] = "decision-" + "a" * 64

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    with pytest.raises(GateError, match="different route_id"):
        advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)


def test_semantic_gate_accepts_exact_resolution_and_originating_confirmation() -> None:
    results = _results()
    finding = {
        "id": "GENERAL-R1-001",
        "summary": "fixture blocker",
        "defect": "fixture defect",
        "consequence": "fixture consequence",
        "corrective_direction": "fix it",
        "required_evidence": ["same-head confirmation"],
        "status": "OPEN",
    }
    results["review-general-1"]["outcome"] = "REQUEST_CHANGES"
    results["review-general-1"]["blocking_findings"] = [finding]
    head = results["remediate"]["head_sha"]
    results["remediate"]["finding_resolutions"] = [
        {
            "finding_id": finding["id"],
            "resolution_commit": head,
            "resolved_head_sha": head,
            "resolution_summary": "fixed",
            "tests": ["fixture regression"],
        }
    ]
    results["review-general-2"]["finding_confirmations"] = [
        {
            "finding_id": finding["id"],
            "status": "CONFIRMED_RESOLVED",
            "reviewed_fix_sha": head,
            "evidence": ["fixture regression passed"],
        }
    ]
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        if key == "gate:review-general-2":
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "task": {"id": task_id, "status": "blocked"},
                        "runs": [],
                        "events": [{"kind": "created"}],
                    }
                ),
                "",
            )
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {
                "id": task_id,
                "status": "blocked" if key == "final-review" else "done",
            },
            "runs": [] if key == "final-review" else [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    result = advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)

    assert result["advanced"] == ["review-general-2", "final-review"]
    general_gate_completion = next(
        command
        for command in commands
        if "complete" in command and "t-gate-review-general-2" in command
    )
    activation = json.loads(
        general_gate_completion[general_gate_completion.index("--result") + 1]
    )
    assert activation["reviewed_head_sha"] == head
    assert activation["originating_findings"] == [finding]


def test_semantic_gate_validates_final_result_before_human_disposition() -> None:
    results = _results()
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "create" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "id": "t_disposition",
                        "status": "blocked",
                        "body": command[command.index("--body") + 1],
                    }
                ),
                "",
            )
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    result = advance_gates(
        _route(),
        _task_ids(),
        board="pip-mdk",
        runner=runner,
        final_evidence_validator=_live_evidence,
        final_snapshot_validator=_final_snapshot,
    )

    assert result["waiting_for"] == "human-review-and-merge"
    subscribe_index = next(
        index for index, command in enumerate(commands) if "notify-subscribe" in command
    )
    assert all("show" in command for command in commands[:7])
    assert subscribe_index >= 7
    assert not any(command[1:2] == ["send"] for command in commands)
    disposition_create = next(command for command in commands if "create" in command)
    disposition_key = disposition_create[
        disposition_create.index("--idempotency-key") + 1
    ]
    assert str(results["final-review"]["pr_number"]) in disposition_key
    assert str(results["final-review"]["reviewed_head_sha"]) in disposition_key


@pytest.mark.parametrize(
    ("outcome", "waiting_for"),
    [
        ("RETURN_TO_BUILD", "held-return-to-build"),
        ("RETURN_TO_REVIEW", "held-return-to-review"),
        ("RETURN_TO_PLANNING", "held-return-to-planning"),
        ("WAIT_FOR_ISSUE_CREATOR", "held-wait-for-issue-creator"),
        ("BLOCKED", "held-blocked"),
        ("ABANDON", "abandoned"),
    ],
)
def test_valid_nonclean_final_outcome_enters_durable_hold(
    outcome: str, waiting_for: str
) -> None:
    results = _results()
    results["final-review"]["outcome"] = outcome

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        if "create" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "id": "t_disposition",
                        "status": "blocked",
                        "body": command[command.index("--body") + 1],
                    }
                ),
                "",
            )
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    result = advance_gates(
        _route(),
        _task_ids(),
        board="pip-mdk",
        runner=runner,
        before_activate=lambda: (_ for _ in ()).throw(
            AssertionError("must not revalidate")
        ),
    )

    assert result["waiting_for"] == waiting_for
    assert result["final_outcome"] == outcome


def test_invalid_final_metadata_never_notifies_human() -> None:
    results = _results()
    results["final-review"]["reviewed_head_sha"] = "d" * 40
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if "create" in command:
            return subprocess.CompletedProcess(
                command,
                0,
                json.dumps(
                    {
                        "id": "t_disposition",
                        "status": "blocked",
                        "body": command[command.index("--body") + 1],
                    }
                ),
                "",
            )
        if "show" not in command:
            return subprocess.CompletedProcess(command, 0, "", "")
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    with pytest.raises(GateError, match="different head"):
        advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)

    assert not any(command[1:2] == ["send"] for command in commands)


def test_validated_final_uses_durable_kanban_notification_subscription() -> None:
    commands: list[list[str]] = []
    result = _results()["final-review"]

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        output = (
            json.dumps(
                {
                    "id": "t_disposition",
                    "status": "blocked",
                    "body": command[command.index("--body") + 1],
                }
            )
            if "create" in command
            else ""
        )
        return subprocess.CompletedProcess(command, 0, output, "")

    kanban_gate._notify_validated_final("pip-mdk", result, runner)

    assert [
        next(
            verb
            for verb in ("create", "notify-subscribe", "complete")
            if verb in command
        )
        for command in commands
    ] == ["create", "notify-subscribe", "complete"]
    assert all("t_disposition" in command for command in commands[1:])
    assert "483923125" in commands[1]


def test_builder_return_to_planning_creates_deterministic_replan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    results = _results()
    results["build"]["outcome"] = "RETURN_TO_PLANNING"
    results["build"]["github_ci_green"] = False
    results["build"]["evidence"] = {
        "incompatibility": {
            "reason": "required extension point was removed",
            "observations": ["current tree has no ProjectionSink"],
        }
    }
    monkeypatch.setattr(
        kanban_gate,
        "_route_builder_replan",
        lambda route, *, board, runner, before_activate: {"replan": "t-replan"},
    )

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {
                "id": task_id,
                "status": "done" if key == "build" else "blocked",
            },
            "runs": [{"metadata": results[key]}] if key == "build" else [],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    result = advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)

    assert result == {
        "advanced": [],
        "waiting_for": "replanning",
        "replan_task": "t-replan",
    }


def test_schema_valid_blocked_review_routes_to_durable_hold(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    results = _results()
    results["review-general-1"]["outcome"] = "BLOCKED"
    monkeypatch.setattr(
        kanban_gate,
        "_notify_held_result",
        lambda board, result, runner: "t-held-review",
    )

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    assert advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner) == {
        "advanced": [],
        "waiting_for": "human-held",
        "held_task": "t-held-review",
    }


def test_remediation_return_to_planning_creates_deterministic_replan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    results = _results()
    results["review-general-1"]["outcome"] = "REQUEST_CHANGES"
    results["review-general-1"]["blocking_findings"] = [
        {
            "id": "GENERAL-R1-009",
            "summary": "fixture blocker",
            "defect": "fixture defect",
            "consequence": "fixture impact",
            "required_evidence": ["fixture evidence"],
            "corrective_direction": "fixture fix",
            "status": "OPEN",
        }
    ]
    results["remediate"]["outcome"] = "RETURN_TO_PLANNING"
    results["remediate"]["github_ci_green"] = False
    results["remediate"]["evidence"] = {
        "incompatibility": {
            "reason": "required extension point was removed",
            "observations": ["current tree has no ProjectionSink"],
        }
    }
    monkeypatch.setattr(
        kanban_gate,
        "_route_builder_replan",
        lambda route, *, board, runner, before_activate: {"replan": "t-replan-2"},
    )

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        task_id = command[command.index("show") + 1]
        key = next(key for key, value in _task_ids().items() if value == task_id)
        gate_response = _gate_response(command, key, task_id)
        if gate_response is not None:
            return gate_response
        payload = {
            "task": {"id": task_id, "status": "done"},
            "runs": [{"metadata": results[key]}],
            "events": [{"kind": "created"}],
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    result = advance_gates(_route(), _task_ids(), board="pip-mdk", runner=runner)
    assert result["waiting_for"] == "replanning"
    assert result["replan_task"] == "t-replan-2"
