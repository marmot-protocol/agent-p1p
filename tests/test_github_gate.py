from __future__ import annotations

import json
import subprocess

import pytest

from pip_agent.github_gate import (
    GitHubGateError,
    fetch_live_final_evidence,
    validate_live_final_snapshot,
    validate_live_implementation_base,
)

HEAD = "a" * 40
BASE = "b" * 40


def test_implementation_base_must_be_ancestor_of_head_and_master() -> None:
    commands: list[list[str]] = []

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps({"status": "ahead", "merge_base_commit": {"sha": BASE}}),
            "",
        )

    validate_live_implementation_base(BASE, HEAD, runner=runner)

    assert len(commands) == 2
    assert commands[0][-1].endswith(f"compare/{BASE}...{HEAD}")
    assert commands[1][-1].endswith(f"compare/{BASE}...master")


def test_implementation_base_rejects_unrelated_commit() -> None:
    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps({"status": "diverged", "merge_base_commit": {"sha": "c" * 40}}),
            "",
        )

    with pytest.raises(GitHubGateError, match="not an ancestor"):
        validate_live_implementation_base(BASE, HEAD, runner=runner)


def _runner(
    *,
    head: str = HEAD,
    conclusion: str = "success",
    intake_label: bool = True,
    review_decision: str | None = None,
    reviews: list[dict[str, object]] | None = None,
    statuses: list[dict[str, object]] | None = None,
):
    def run(command: list[str]) -> subprocess.CompletedProcess[str]:
        if "graphql" in command:
            payload = {
                "data": {
                    "repository": {
                        "issue": {
                            "number": 1240,
                            "state": "OPEN",
                            "labels": {
                                "nodes": ([{"name": "pip-ok"}] if intake_label else []),
                                "pageInfo": {"hasNextPage": False},
                            },
                        },
                        "pullRequest": {
                            "number": 42,
                            "state": "OPEN",
                            "isDraft": True,
                            "mergeable": "MERGEABLE",
                            "headRefName": "pip/mdk-1240",
                            "headRefOid": head,
                            "reviewDecision": review_decision,
                            "author": {"login": "agent-p1p"},
                            "reviewThreads": {
                                "nodes": [{"isResolved": True}],
                                "pageInfo": {"hasNextPage": False},
                            },
                            "reviews": {
                                "nodes": [
                                    dict(
                                        review,
                                        databaseId=review.get("databaseId", index + 1),
                                    )
                                    for index, review in enumerate(reviews or [])
                                ],
                                "pageInfo": {"hasPreviousPage": False},
                            },
                        },
                    }
                }
            }
        elif "check-runs" in " ".join(command):
            payload = {
                "total_count": 1,
                "check_runs": [
                    {
                        "head_sha": HEAD,
                        "status": "completed",
                        "conclusion": conclusion,
                    }
                ],
            }
        else:
            payload = (
                statuses
                if statuses is not None
                else [
                    {
                        "id": 1,
                        "sha": HEAD,
                        "state": "success",
                        "context": "ci/default",
                        "updated_at": "2026-08-16T20:00:00Z",
                    }
                ]
            )
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    return run


def test_live_final_evidence_requires_exact_open_draft_owned_green_pr() -> None:
    evidence = fetch_live_final_evidence(42, HEAD, runner=_runner())

    assert evidence["pip_owned"] is True
    assert evidence["open_blocking_threads"] == 0
    assert evidence["ci"]["required_checks_green"] is True


def test_live_final_evidence_rejects_changed_head() -> None:
    with pytest.raises(GitHubGateError, match="head changed"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(head="b" * 40))


def test_live_final_evidence_rejects_any_non_green_check_attempt() -> None:
    with pytest.raises(GitHubGateError, match="non-green"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(conclusion="failure"))


def test_commit_status_uses_latest_attempt_per_context() -> None:
    historical_failure_then_success = [
        {
            "id": 2,
            "sha": HEAD,
            "state": "success",
            "context": "ci/legacy",
            "updated_at": "2026-08-16T20:01:00Z",
        },
        {
            "id": 1,
            "sha": HEAD,
            "state": "failure",
            "context": "ci/legacy",
            "updated_at": "2026-08-16T20:00:00Z",
        },
    ]
    assert fetch_live_final_evidence(
        42, HEAD, runner=_runner(statuses=historical_failure_then_success)
    )["ci"]["required_checks_green"]

    latest_failure = [
        dict(historical_failure_then_success[0], id=1),
        dict(
            historical_failure_then_success[1], id=2, updated_at="2026-08-16T20:02:00Z"
        ),
    ]
    with pytest.raises(GitHubGateError, match="commit status"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(statuses=latest_failure))


@pytest.mark.parametrize(
    "override",
    [
        {"updated_at": ""},
        {"updated_at": "not-a-timestamp"},
        {"context": "   "},
        {"state": "bogus"},
    ],
)
def test_commit_status_rejects_malformed_rows(override: dict[str, object]) -> None:
    status = {
        "id": 1,
        "sha": HEAD,
        "state": "success",
        "context": "ci/default",
        "updated_at": "2026-08-16T20:00:00Z",
        **override,
    }
    with pytest.raises(GitHubGateError, match="malformed"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(statuses=[status]))


def test_commit_status_paginates_before_selecting_latest_context() -> None:
    base = _runner()
    older = [
        {
            "id": index + 1,
            "sha": HEAD,
            "state": "success",
            "context": f"ci/context-{index}",
            "updated_at": "2026-08-16T20:00:00Z",
        }
        for index in range(100)
    ]
    older[0]["state"] = "failure"
    later_page = [
        {
            "id": 101,
            "sha": HEAD,
            "state": "success",
            "context": "ci/context-0",
            "updated_at": "2026-08-16T20:01:00Z",
        }
    ]

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        endpoint = command[-1]
        if "/statuses?" in endpoint:
            payload = older if endpoint.endswith("page=1") else later_page
            return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")
        return base(command)

    assert fetch_live_final_evidence(42, HEAD, runner=runner)["ci"][
        "required_checks_green"
    ]


def test_live_final_evidence_rejects_withdrawn_issue_intake() -> None:
    with pytest.raises(GitHubGateError, match="pip-ok"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(intake_label=False))


@pytest.mark.parametrize("conclusion", ["neutral", "skipped"])
def test_live_final_evidence_rejects_non_success_check(conclusion: str) -> None:
    with pytest.raises(GitHubGateError, match="non-green"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(conclusion=conclusion))


def test_live_final_evidence_rejects_effective_changes_requested() -> None:
    with pytest.raises(GitHubGateError, match="blocking GitHub review"):
        fetch_live_final_evidence(
            42, HEAD, runner=_runner(review_decision="CHANGES_REQUESTED")
        )


def test_live_final_evidence_rejects_old_head_changes_request_until_reconfirmed() -> (
    None
):
    requested = {
        "state": "CHANGES_REQUESTED",
        "submittedAt": "2026-08-16T20:00:00Z",
        "author": {"login": "reviewer"},
    }
    with pytest.raises(GitHubGateError, match="undismissed"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(reviews=[requested]))

    approved = {
        "state": "APPROVED",
        "submittedAt": "2026-08-16T20:01:00Z",
        "author": {"login": "reviewer"},
    }
    assert (
        fetch_live_final_evidence(
            42, HEAD, runner=_runner(reviews=[requested, approved])
        )["pip_owned"]
        is True
    )


def test_live_final_evidence_ignores_dismissed_review() -> None:
    dismissed = {
        "state": "DISMISSED",
        "submittedAt": "2026-08-16T20:00:00Z",
        "author": {"login": "reviewer"},
    }
    assert (
        fetch_live_final_evidence(42, HEAD, runner=_runner(reviews=[dismissed]))[
            "pip_owned"
        ]
        is True
    )


def test_actionable_review_rejects_malformed_timestamp() -> None:
    reviews = [
        {
            "databaseId": 100,
            "state": "CHANGES_REQUESTED",
            "submittedAt": "2026-08-16T20:00:00Z",
            "author": {"login": "reviewer"},
        },
        {
            "databaseId": 101,
            "state": "APPROVED",
            "submittedAt": "not-a-timestamp",
            "author": {"login": "reviewer"},
        },
    ]
    with pytest.raises(GitHubGateError, match="timestamp evidence is malformed"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(reviews=reviews))


def test_equal_timestamp_uses_monotonic_review_id() -> None:
    reviews = [
        {
            "databaseId": 100,
            "state": "APPROVED",
            "submittedAt": "2026-08-16T20:00:00Z",
            "author": {"login": "reviewer"},
        },
        {
            "databaseId": 101,
            "state": "CHANGES_REQUESTED",
            "submittedAt": "2026-08-16T20:00:00Z",
            "author": {"login": "reviewer"},
        },
    ]
    with pytest.raises(GitHubGateError, match="undismissed"):
        fetch_live_final_evidence(42, HEAD, runner=_runner(reviews=reviews))


def test_final_snapshot_refetches_ci_and_graphql_after_ci() -> None:
    calls: list[list[str]] = []
    base = _runner()

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        calls.append(command)
        return base(command)

    validate_live_final_snapshot(42, HEAD, runner=runner)
    assert sum("graphql" in command for command in calls) == 2
    assert any("check-runs" in " ".join(command) for command in calls)
    assert any("/statuses?" in command[-1] for command in calls)
    assert all("--slurp" not in command for command in calls)


def test_live_final_evidence_refetches_authorization_after_ci_queries() -> None:
    graphql_calls = 0

    def runner(command: list[str]) -> subprocess.CompletedProcess[str]:
        nonlocal graphql_calls
        if "graphql" in command:
            graphql_calls += 1
        return _runner(intake_label=graphql_calls < 2)(command)

    with pytest.raises(GitHubGateError, match="authorization snapshot"):
        fetch_live_final_evidence(42, HEAD, runner=runner)
    assert graphql_calls == 2
