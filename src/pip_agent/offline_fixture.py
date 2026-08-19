from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path
from typing import Any, Literal

from .case_store import CaseStore
from .contracts import ContractError, assert_exact_head_evidence, validate_contract
from .state_machine import CaseState

Fault = Literal["wrong-model", "stale-head", "red-ci", "malformed-result"]


class FixtureFailure(RuntimeError):
    """The offline workflow fixture failed closed as intended or unexpectedly."""


CASE_ID = "mdk#900001"
PR_NUMBER = 900001
PLAN_VERSION = 1
HEAD = "b" * 40
SKILLS_COMMIT = "a" * 40
STAMP_START = "2026-08-15T12:00:00Z"
STAMP_END = "2026-08-15T12:10:00Z"
ROUTE_ID = "decision-" + "d" * 64
DECISION_BODY_SHA = "e" * 64
PLANNER_BODY_SHA = "f" * 64
PLANNED_BASE_SHA = "c" * 40


def _common(task_id: str, role: str, model: str) -> dict[str, Any]:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": CASE_ID,
        "task_id": task_id,
        "role": role,
        "requested_model": model,
        "actual_model": model,
        "skills_repository_commit": SKILLS_COMMIT,
        "started_at": STAMP_START,
        "completed_at": STAMP_END,
        "evidence": {"fixture": True},
    }
    if role != "planner":
        result.update(
            {
                "route_id": ROUTE_ID,
                "comment_id": 2,
                "evidence_body_sha256": DECISION_BODY_SHA,
                "planner_comment_id": 1,
                "planner_body_sha256": PLANNER_BODY_SHA,
                "planned_base_sha": PLANNED_BASE_SHA,
            }
        )
    return result


def _planner() -> dict[str, Any]:
    return {
        **_common("plan-v1", "planner", "openai-codex/gpt-5.6-sol"),
        "outcome": "PROCEED",
        "plan_version": 1,
        "planned_base_sha": "c" * 40,
        "root_cause": "offline fixture",
        "authorized_scope": "repair repository-local fixture behavior",
        "sensitive_scope": [],
        "dependencies": [],
        "open_decisions": [],
        "plan_file": "artifacts/plan-v1.md",
        "issue_comment_url": "https://github.com/marmot-protocol/mdk/issues/900001#issuecomment-1",
    }


def _builder() -> dict[str, Any]:
    return {
        **_common("build-r1", "builder-grok", "cursor/cursor-grok-4.6-high"),
        "outcome": "REVIEW_READY",
        "plan_version": 1,
        "build_round": 1,
        "implementation_base_sha": HEAD,
        "pr_number": PR_NUMBER,
        "head_sha": HEAD,
        "ci_head_sha": HEAD,
        "github_ci_green": True,
        "local_checks": ["offline fixture"],
        "finding_resolutions": [],
    }


def _review(role: str, model: str) -> dict[str, Any]:
    return {
        **_common(f"{role}-r1", role, model),
        "outcome": "APPROVE",
        "review_round": 1,
        "plan_version": 1,
        "pr_number": PR_NUMBER,
        "reviewed_head_sha": HEAD,
        "blocking_findings": [],
        "suggestions": [],
        "finding_confirmations": [],
    }


def _final() -> dict[str, Any]:
    return {
        **_common("final-review-r1", "final-reviewer", "openai-codex/gpt-5.6-sol"),
        "outcome": "HUMAN_REVIEW_REQUIRED",
        "final_review_round": 1,
        "plan_version": 1,
        "pr_number": PR_NUMBER,
        "reviewed_head_sha": HEAD,
        "residual_uncertainties": [],
        "decision_rationale": "Offline exact-head fixture passed all deterministic gates.",
    }


def run_offline_fixture(database: Path, fault: Fault | None = None) -> dict[str, Any]:
    store = CaseStore(database)
    store.create_case(CASE_ID, "marmot-protocol/mdk", 900001, "pip-ok")
    state = CaseState.PLANNING
    try:
        planner = _planner()
        if fault == "malformed-result":
            planner.pop("role")
        validate_contract("planner-result", planner)
        store.append_run(CASE_ID, planner["task_id"], "planner", 1, planner)
        store.record_plan_version(CASE_ID, 0, PLAN_VERSION)
        state = store.set_state(
            CASE_ID, state.value, planner["outcome"], "planner-result"
        )

        builder = _builder()
        if fault == "wrong-model":
            builder["actual_model"] = "cursor/auto"
        if fault == "red-ci":
            builder["github_ci_green"] = False
        validate_contract("builder-result", builder)
        store.append_run(CASE_ID, builder["task_id"], "builder-grok", 1, builder)
        store.bind_pr_head(CASE_ID, PR_NUMBER, HEAD)
        state = store.set_state(
            CASE_ID, state.value, builder["outcome"], "builder-result"
        )

        general = _review("reviewer-general", "openai-codex/gpt-5.6-sol")
        secperf = _review("reviewer-secperf", "cursor/kimi-k3-high")
        if fault == "stale-head":
            secperf["reviewed_head_sha"] = "d" * 40
        for review in (general, secperf):
            validate_contract("review-result", review)
            store.append_run(CASE_ID, review["task_id"], review["role"], 1, review)
        evidence = {
            "builder_result": builder,
            "general_review": general,
            "secperf_review": secperf,
            "mandatory_findings": [],
            "ci": {
                "head_sha": HEAD,
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
        assert_exact_head_evidence(CASE_ID, PR_NUMBER, 1, HEAD, evidence)
        state = store.set_state(
            CASE_ID, state.value, "REVIEWS_APPROVED", "exact-head-join"
        )

        final = _final()
        validate_contract("final-result", final)
        if final["case_id"] != CASE_ID or final["pr_number"] != PR_NUMBER:
            raise ContractError("final review case or PR mismatch")
        if final["reviewed_head_sha"] != HEAD:
            raise ContractError("final review is stale")
        store.append_run(CASE_ID, final["task_id"], "final-reviewer", 1, final)
        state = store.set_state(
            CASE_ID, state.value, final["outcome"], "final-review-result"
        )
    except (ContractError, ValueError) as exc:
        raise FixtureFailure(str(exc)) from exc

    return {
        "case_id": CASE_ID,
        "state": state.value,
        "run_ids": [run["task_id"] for run in store.list_runs(CASE_ID)],
        "merge_performed": False,
        "shadow_mode": True,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Run Pip v2 offline workflow fixture")
    parser.add_argument("--database", type=Path)
    parser.add_argument(
        "--fault",
        choices=("wrong-model", "stale-head", "red-ci", "malformed-result"),
    )
    args = parser.parse_args()
    if args.database:
        result = run_offline_fixture(args.database, args.fault)
    else:
        with tempfile.TemporaryDirectory(prefix="pip-v2-fixture-") as directory:
            result = run_offline_fixture(Path(directory) / "cases.db", args.fault)
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
