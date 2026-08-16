from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
from typing import Any

from .contracts import ContractError, assert_exact_head_evidence, validate_contract
from .kanban_router import RouteError, canonical_route_id, route_once

Runner = Callable[[list[str]], subprocess.CompletedProcess[str]]
EXPECTED_MODELS = {
    "builder-grok": {"cursor-grok-4.6-high", "cursor/cursor-grok-4.6-high"},
    "reviewer-general": {"gpt-5.6-sol", "openai-codex/gpt-5.6-sol"},
    "reviewer-secperf": {"kimi-k3-high", "cursor/kimi-k3-high"},
    "final-reviewer": {"gpt-5.6-sol", "openai-codex/gpt-5.6-sol"},
}


class GateError(RouteError):
    """A Kanban task result does not satisfy the next deterministic gate."""


def _command_json(command: list[str], runner: Runner) -> Any:
    completed = runner(command)
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise GateError(f"Kanban gate command failed: {detail}")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise GateError("Kanban gate command returned malformed JSON") from exc


def _show(board: str, task_id: str, runner: Runner) -> dict[str, Any]:
    payload = _command_json(
        ["hermes", "kanban", "--board", board, "show", task_id, "--json"],
        runner,
    )
    if not isinstance(payload, dict) or not isinstance(payload.get("task"), dict):
        raise GateError("Kanban show response is malformed")
    return payload


def _metadata(show: dict[str, Any], contract: str) -> dict[str, Any]:
    runs = show.get("runs")
    if not isinstance(runs, list) or not runs or not isinstance(runs[-1], dict):
        raise GateError("completed task has no durable run result")
    raw = runs[-1].get("metadata")
    if isinstance(raw, str):
        try:
            raw = json.loads(raw)
        except json.JSONDecodeError as exc:
            raise GateError("task run metadata is malformed JSON") from exc
    if not isinstance(raw, dict):
        raise GateError("task run metadata is not an object")
    try:
        validate_contract(contract, raw)
    except ContractError as exc:
        raise GateError(f"task run metadata fails {contract}: {exc}") from exc
    task = show["task"]
    if raw.get("task_id") != task.get("id"):
        raise GateError("task result is bound to a different task")
    return raw


def _assert_common(result: dict[str, Any], route: dict[str, Any]) -> None:
    for field in (
        "case_id",
        "route_id",
        "comment_id",
        "evidence_body_sha256",
        "planner_comment_id",
        "planner_body_sha256",
        "planned_base_sha",
        "plan_version",
    ):
        if result.get(field) != route.get(field):
            raise GateError(f"task result is bound to a different {field}")


def _index_unique(
    items: list[dict[str, Any]], key: str, *, label: str
) -> dict[str, dict[str, Any]]:
    indexed: dict[str, dict[str, Any]] = {}
    for item in items:
        identifier = item[key]
        if identifier in indexed:
            raise GateError(f"duplicate {label} identifier: {identifier}")
        indexed[identifier] = item
    return indexed


def _assert_expected_model(result: dict[str, Any], role: str) -> None:
    expected = EXPECTED_MODELS[role]
    if (
        result.get("requested_model") not in expected
        or result.get("actual_model") not in expected
    ):
        raise GateError(f"{role} result reports an unauthorized model")


def _builder_result(
    show: dict[str, Any], route: dict[str, Any], *, expected_round: int
) -> dict[str, Any]:
    result = _metadata(show, "builder-result")
    _assert_common(result, route)
    _assert_expected_model(result, "builder-grok")
    if result.get("outcome") != "REVIEW_READY":
        raise GateError(f"builder did not return REVIEW_READY: {result.get('outcome')}")
    if result.get("build_round") != expected_round:
        raise GateError("builder result has the wrong build round")
    if result.get("github_ci_green") is not True:
        raise GateError("builder result does not attest green GitHub CI")
    if result.get("head_sha") != result.get("ci_head_sha"):
        raise GateError("builder result CI is not bound to its exact head")
    return result


def _route_builder_replan(
    route: dict[str, Any],
    *,
    board: str,
    runner: Runner,
    before_activate: Callable[[], None] | None,
) -> dict[str, str]:
    replanning = {
        **route,
        "action": "replan",
        "state": "PLANNING",
        "replan_reason": "builder_return_to_planning",
    }
    replanning.pop("route_id", None)
    replanning["route_id"] = canonical_route_id(replanning)
    return route_once(
        replanning,
        board=board,
        runner=runner,
        before_activate=before_activate,
    )


def _review_result(
    show: dict[str, Any],
    route: dict[str, Any],
    *,
    expected_role: str,
    expected_round: int,
    expected_pr: int,
    expected_head: str,
    require_approval: bool,
) -> dict[str, Any]:
    result = _metadata(show, "review-result")
    _assert_common(result, route)
    if result.get("role") != expected_role:
        raise GateError("review result has the wrong role")
    _assert_expected_model(result, expected_role)
    if result.get("review_round") != expected_round:
        raise GateError("review result has the wrong review round")
    if result.get("pr_number") != expected_pr:
        raise GateError("review result is bound to a different PR")
    if result.get("reviewed_head_sha") != expected_head:
        raise GateError("review result is bound to a different head")
    outcome = result.get("outcome")
    if outcome not in {"APPROVE", "REQUEST_CHANGES"}:
        raise GateError(f"review did not complete semantically: {outcome}")
    if outcome == "REQUEST_CHANGES" and not result.get("blocking_findings"):
        raise GateError("REQUEST_CHANGES review has no blocking findings")
    if require_approval and (
        outcome != "APPROVE" or bool(result.get("blocking_findings"))
    ):
        raise GateError("same-head review still has blocking findings")
    return result


def _final_result(
    show: dict[str, Any],
    route: dict[str, Any],
    *,
    expected_pr: int,
    expected_head: str,
) -> dict[str, Any]:
    result = _metadata(show, "final-result")
    _assert_common(result, route)
    if result.get("outcome") != "BLOCKED_UNEXPECTED_MODEL":
        _assert_expected_model(result, "final-reviewer")
    if result.get("role") != "final-reviewer":
        raise GateError("final result has the wrong role")
    if result.get("pr_number") != expected_pr:
        raise GateError("final result is bound to a different PR")
    if result.get("reviewed_head_sha") != expected_head:
        raise GateError("final result is bound to a different head")
    return result


def _is_done(show: dict[str, Any]) -> bool:
    return show["task"].get("status") in {"done", "archived"}


def _initial_gate_is_blocked(show: dict[str, Any]) -> bool:
    if show["task"].get("status") != "blocked":
        return False
    if show.get("runs"):
        return False
    events = show.get("events")
    return isinstance(events, list) and not any(
        isinstance(event, dict) and event.get("kind") in {"blocked", "unblocked"}
        for event in events
    )


def _release_gate(
    board: str,
    gate_show: dict[str, Any],
    runner: Runner,
    before_activate: Callable[[], None] | None = None,
) -> bool:
    if not _initial_gate_is_blocked(gate_show):
        return False
    gate_id = gate_show["task"]["id"]
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
            "semantic and live authorization gates passed",
        ]
    )
    if completed.returncode != 0:
        raise GateError("Kanban semantic activation-gate completion failed")
    return True


def _notify_validated_final(board: str, result: dict[str, Any], runner: Runner) -> None:
    outcome = result["outcome"]
    pr_number = result["pr_number"]
    head = result["reviewed_head_sha"]
    disposition_binding = json.dumps(
        {
            "case_id": result["case_id"],
            "outcome": outcome,
            "pr_number": pr_number,
            "reviewed_head_sha": head,
            "route_id": result["route_id"],
        },
        sort_keys=True,
    )
    disposition_body = (
        f"{disposition_binding}\n"
        f"Validated final disposition for {result['case_id']} at {head}. "
        f"Human review and merge remain mandatory. "
        f"https://github.com/marmot-protocol/mdk/pull/{pr_number}"
    )
    disposition = runner(
        [
            "hermes",
            "kanban",
            "--board",
            board,
            "create",
            f"Pip v2: {outcome} for MDK PR #{pr_number}",
            "--body",
            disposition_body,
            "--idempotency-key",
            f"pip-v2-final-disposition:{result['route_id']}:{outcome}:{pr_number}:{head}",
            "--initial-status",
            "blocked",
            "--created-by",
            "pip-v2-router",
            "--max-retries",
            "1",
            "--json",
        ]
    )
    if disposition.returncode != 0:
        raise GateError("validated final disposition staging failed")
    try:
        task = json.loads(disposition.stdout)
        task_id = task["id"]
        status = task["status"]
    except (json.JSONDecodeError, KeyError, TypeError) as exc:
        raise GateError(
            "validated final disposition returned malformed task data"
        ) from exc
    if not isinstance(task_id, str) or not task_id.startswith("t_"):
        raise GateError("validated final disposition returned an invalid task id")
    if task.get("body") != disposition_body:
        raise GateError("validated final disposition metadata did not match")
    if status == "done":
        return
    if status != "blocked":
        raise GateError("validated final disposition task has an unsafe status")
    subscribed = runner(
        [
            "hermes",
            "kanban",
            "--board",
            board,
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
        raise GateError("validated final notification subscription failed")
    completed = runner(
        [
            "hermes",
            "kanban",
            "--board",
            board,
            "complete",
            task_id,
            "--result",
            f"{outcome}: human review required for PR #{pr_number} at {head}",
            "--summary",
            f"https://github.com/marmot-protocol/mdk/pull/{pr_number}",
            "--metadata",
            json.dumps(result, sort_keys=True, separators=(",", ":")),
        ]
    )
    if completed.returncode != 0:
        raise GateError("validated final disposition completion failed")


def advance_gates(
    route: dict[str, Any],
    task_ids: dict[str, str],
    *,
    board: str,
    runner: Runner,
    before_activate: Callable[[], None] | None = None,
    final_evidence_validator: Callable[[int, str], dict[str, Any]] | None = None,
    final_snapshot_validator: Callable[[int, str], None] | None = None,
) -> dict[str, Any]:
    shows = {key: _show(board, task_id, runner) for key, task_id in task_ids.items()}
    advanced: list[str] = []

    build = shows["build"]
    if not _is_done(build):
        return {"advanced": advanced, "waiting_for": "build"}
    first_outcome = _metadata(build, "builder-result")
    _assert_common(first_outcome, route)
    _assert_expected_model(first_outcome, "builder-grok")
    if first_outcome.get("outcome") == "RETURN_TO_PLANNING":
        replanning = _route_builder_replan(
            route,
            board=board,
            runner=runner,
            before_activate=before_activate,
        )
        return {
            "advanced": advanced,
            "waiting_for": "replanning",
            "replan_task": replanning["replan"],
        }
    first_build = _builder_result(build, route, expected_round=1)
    for key in ("review-general-1", "review-secperf-1"):
        if _release_gate(board, shows[f"gate:{key}"], runner, before_activate):
            advanced.append(key)

    first_reviews = (shows["review-general-1"], shows["review-secperf-1"])
    if not all(_is_done(show) for show in first_reviews):
        return {"advanced": advanced, "waiting_for": "review-round-1"}
    first_general = _review_result(
        first_reviews[0],
        route,
        expected_role="reviewer-general",
        expected_round=1,
        expected_pr=first_build["pr_number"],
        expected_head=first_build["head_sha"],
        require_approval=False,
    )
    first_secperf = _review_result(
        first_reviews[1],
        route,
        expected_role="reviewer-secperf",
        expected_round=1,
        expected_pr=first_build["pr_number"],
        expected_head=first_build["head_sha"],
        require_approval=False,
    )
    if _release_gate(board, shows["gate:remediate"], runner, before_activate):
        advanced.append("remediate")

    remediation = shows["remediate"]
    if not _is_done(remediation):
        return {"advanced": advanced, "waiting_for": "remediation"}
    remediation_outcome = _metadata(remediation, "builder-result")
    _assert_common(remediation_outcome, route)
    _assert_expected_model(remediation_outcome, "builder-grok")
    if remediation_outcome.get("outcome") == "RETURN_TO_PLANNING":
        replanning = _route_builder_replan(
            route,
            board=board,
            runner=runner,
            before_activate=before_activate,
        )
        return {
            "advanced": advanced,
            "waiting_for": "replanning",
            "replan_task": replanning["replan"],
        }
    remediated = _builder_result(remediation, route, expected_round=2)
    if remediated["pr_number"] != first_build["pr_number"]:
        raise GateError("remediation changed the authorized PR")
    general_findings = _index_unique(
        first_general["blocking_findings"], "id", label="general finding"
    )
    secperf_findings = _index_unique(
        first_secperf["blocking_findings"], "id", label="security finding"
    )
    if set(general_findings) & set(secperf_findings):
        raise GateError("round-1 reviewers emitted duplicate finding identifiers")
    findings_by_role = {
        "reviewer-general": set(general_findings),
        "reviewer-secperf": set(secperf_findings),
    }
    required_findings = set().union(*findings_by_role.values())
    resolutions = _index_unique(
        remediated["finding_resolutions"], "finding_id", label="resolution"
    )
    if set(resolutions) != required_findings:
        raise GateError("remediation does not resolve the exact round-1 finding set")
    if any(
        resolution["resolved_head_sha"] != remediated["head_sha"]
        for resolution in resolutions.values()
    ):
        raise GateError("finding resolution is bound to a different remediation head")
    for key in ("review-general-2", "review-secperf-2"):
        if _release_gate(board, shows[f"gate:{key}"], runner, before_activate):
            advanced.append(key)

    second_reviews = (shows["review-general-2"], shows["review-secperf-2"])
    if not all(_is_done(show) for show in second_reviews):
        return {"advanced": advanced, "waiting_for": "review-round-2"}
    second_results: dict[str, dict[str, Any]] = {}
    for show, role in zip(
        second_reviews, ("reviewer-general", "reviewer-secperf"), strict=True
    ):
        second_result = _review_result(
            show,
            route,
            expected_role=role,
            expected_round=2,
            expected_pr=remediated["pr_number"],
            expected_head=remediated["head_sha"],
            require_approval=True,
        )
        second_results[role] = second_result
        confirmations = _index_unique(
            second_result["finding_confirmations"],
            "finding_id",
            label=f"{role} confirmation",
        )
        if set(confirmations) != findings_by_role[role]:
            raise GateError(f"{role} did not confirm its exact round-1 finding set")
        if any(
            confirmation["status"] != "CONFIRMED_RESOLVED"
            or confirmation["reviewed_fix_sha"] != remediated["head_sha"]
            for confirmation in confirmations.values()
        ):
            raise GateError(f"{role} retained an unresolved or stale finding")

    final = shows["final-review"]
    if _is_done(final):
        final_result = _final_result(
            final,
            route,
            expected_pr=remediated["pr_number"],
            expected_head=remediated["head_sha"],
        )
        if final_result["outcome"] == "HUMAN_REVIEW_REQUIRED":
            if before_activate is not None:
                before_activate()
            if final_evidence_validator is None:
                raise GateError("live final GitHub verifier is unavailable")
            live_evidence = final_evidence_validator(
                remediated["pr_number"], remediated["head_sha"]
            )
            mandatory_findings = [
                {"id": finding_id, "origin_role": role}
                for role, finding_ids in findings_by_role.items()
                for finding_id in sorted(finding_ids)
            ]
            try:
                assert_exact_head_evidence(
                    str(route["case_id"]),
                    remediated["pr_number"],
                    int(route["plan_version"]),
                    remediated["head_sha"],
                    {
                        "builder_result": remediated,
                        "general_review": second_results["reviewer-general"],
                        "secperf_review": second_results["reviewer-secperf"],
                        "mandatory_findings": mandatory_findings,
                        **live_evidence,
                    },
                )
            except ContractError as exc:
                raise GateError(f"live final join failed: {exc}") from exc
            if before_activate is not None:
                before_activate()
            if final_snapshot_validator is None:
                raise GateError("final GitHub snapshot verifier is unavailable")
            final_snapshot_validator(remediated["pr_number"], remediated["head_sha"])
        _notify_validated_final(board, final_result, runner)
        disposition = {
            "HUMAN_REVIEW_REQUIRED": "human-review-and-merge",
            "RETURN_TO_BUILD": "held-return-to-build",
            "RETURN_TO_REVIEW": "held-return-to-review",
            "RETURN_TO_PLANNING": "held-return-to-planning",
            "WAIT_FOR_ISSUE_CREATOR": "held-wait-for-issue-creator",
            "BLOCKED": "held-blocked",
            "ABANDON": "abandoned",
            "BLOCKED_UNEXPECTED_MODEL": "held-unexpected-model",
        }[final_result["outcome"]]
        return {
            "advanced": advanced,
            "waiting_for": disposition,
            "final_outcome": final_result["outcome"],
        }
    if _release_gate(board, shows["gate:final-review"], runner, before_activate):
        advanced.append("final-review")
    return {"advanced": advanced, "waiting_for": "final-review"}
