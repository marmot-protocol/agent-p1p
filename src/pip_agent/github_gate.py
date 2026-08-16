from __future__ import annotations

import json
import re
import subprocess
from collections.abc import Callable, Mapping
from datetime import datetime
from typing import Any

Runner = Callable[[list[str]], subprocess.CompletedProcess[str]]
REPOSITORY = "marmot-protocol/mdk"
EXPECTED_AUTHOR = "agent-p1p"


class GitHubGateError(RuntimeError):
    """Live GitHub state is incomplete, stale, or unsafe for human handoff."""


_RFC3339_UTC = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z")


def _parse_rfc3339_utc(value: Any) -> datetime:
    if not isinstance(value, str) or _RFC3339_UTC.fullmatch(value) is None:
        raise GitHubGateError("GitHub timestamp evidence is malformed")
    try:
        return datetime.fromisoformat(value.removesuffix("Z") + "+00:00")
    except ValueError as exc:
        raise GitHubGateError("GitHub timestamp evidence is malformed") from exc


def _default_runner(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=False, capture_output=True, text=True)


def _json(command: list[str], runner: Runner) -> Any:
    completed = runner(command)
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise GitHubGateError(f"GitHub query failed closed: {detail}")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise GitHubGateError("GitHub response was not JSON") from exc


def _assert_no_effective_changes_request(review_nodes: Any) -> None:
    if not isinstance(review_nodes, list):
        raise GitHubGateError("GitHub review evidence is malformed")
    latest_reviews: dict[str, tuple[tuple[datetime, int], str]] = {}
    for review in review_nodes:
        if not isinstance(review, Mapping):
            raise GitHubGateError("GitHub review evidence is malformed")
        state = review.get("state")
        if state not in {"APPROVED", "CHANGES_REQUESTED"}:
            continue
        author = review.get("author")
        submitted_at = review.get("submittedAt")
        database_id = review.get("databaseId")
        if (
            not isinstance(author, Mapping)
            or not isinstance(author.get("login"), str)
            or not isinstance(database_id, int)
            or isinstance(database_id, bool)
            or database_id <= 0
        ):
            raise GitHubGateError("actionable GitHub review evidence is malformed")
        login = author["login"]
        order = (_parse_rfc3339_utc(submitted_at), database_id)
        previous = latest_reviews.get(login)
        if previous is None or order > previous[0]:
            latest_reviews[login] = (order, state)
        elif order == previous[0] and state != previous[1]:
            raise GitHubGateError("GitHub review order is ambiguous")
    if any(state == "CHANGES_REQUESTED" for _, state in latest_reviews.values()):
        raise GitHubGateError("an undismissed GitHub changes request remains")


def fetch_live_final_evidence(
    pr_number: int,
    head_sha: str,
    *,
    runner: Runner = _default_runner,
) -> dict[str, Any]:
    query = """
query($owner:String!,$name:String!,$number:Int!){
  repository(owner:$owner,name:$name){
    issue(number:1240){
      number state labels(first:100){nodes{name} pageInfo{hasNextPage}}
    }
    pullRequest(number:$number){
      number state isDraft mergeable headRefName headRefOid reviewDecision
      author{login}
      reviewThreads(first:100){nodes{isResolved} pageInfo{hasNextPage}}
      reviews(last:100){nodes{databaseId state submittedAt author{login}} pageInfo{hasPreviousPage}}
    }
  }
}
""".strip()
    payload = _json(
        [
            "gh",
            "api",
            "graphql",
            "-f",
            f"query={query}",
            "-F",
            "owner=marmot-protocol",
            "-F",
            "name=mdk",
            "-F",
            f"number={pr_number}",
        ],
        runner,
    )
    try:
        repository = payload["data"]["repository"]
        pr = repository["pullRequest"]
        issue = repository["issue"]
    except (KeyError, TypeError) as exc:
        raise GitHubGateError("live PR evidence is missing") from exc
    if (
        not isinstance(issue, Mapping)
        or issue.get("number") != 1240
        or issue.get("state") != "OPEN"
    ):
        raise GitHubGateError("protected issue is missing or closed")
    labels = issue.get("labels")
    if not isinstance(labels, Mapping) or labels.get("pageInfo", {}).get("hasNextPage"):
        raise GitHubGateError("protected issue label evidence is incomplete")
    label_nodes = labels.get("nodes")
    if not isinstance(label_nodes, list) or "pip-ok" not in {
        node.get("name") for node in label_nodes if isinstance(node, Mapping)
    }:
        raise GitHubGateError("protected issue no longer has the pip-ok intake label")
    if not isinstance(pr, Mapping) or pr.get("number") != pr_number:
        raise GitHubGateError("live PR number does not match the authorized PR")
    if pr.get("state") != "OPEN" or pr.get("isDraft") is not True:
        raise GitHubGateError("authorized PR must remain open and draft")
    if pr.get("headRefOid") != head_sha:
        raise GitHubGateError("live PR head changed after review")
    author = pr.get("author")
    if not isinstance(author, Mapping) or author.get("login") != EXPECTED_AUTHOR:
        raise GitHubGateError("PR is not owned by the Pip GitHub identity")
    branch = pr.get("headRefName")
    if not isinstance(branch, str) or not branch.startswith("pip/"):
        raise GitHubGateError("PR head branch is not Pip-owned")
    if pr.get("mergeable") != "MERGEABLE":
        raise GitHubGateError("PR is conflicting or mergeability is unknown")
    if pr.get("reviewDecision") not in {None, "REVIEW_REQUIRED", "APPROVED"}:
        raise GitHubGateError("PR has an effective blocking GitHub review")

    threads = pr.get("reviewThreads")
    if not isinstance(threads, Mapping) or threads.get("pageInfo", {}).get(
        "hasNextPage"
    ):
        raise GitHubGateError("review-thread evidence is incomplete")
    nodes = threads.get("nodes")
    if not isinstance(nodes, list):
        raise GitHubGateError("review-thread evidence is malformed")
    unresolved = sum(
        1
        for thread in nodes
        if not isinstance(thread, Mapping) or not thread.get("isResolved")
    )
    if unresolved:
        raise GitHubGateError("open blocking review threads remain")

    reviews = pr.get("reviews")
    if not isinstance(reviews, Mapping) or reviews.get("pageInfo", {}).get(
        "hasPreviousPage"
    ):
        raise GitHubGateError("GitHub review evidence is incomplete")
    review_nodes = reviews.get("nodes")
    _assert_no_effective_changes_request(review_nodes)

    checks = _json(
        [
            "gh",
            "api",
            f"repos/{REPOSITORY}/commits/{head_sha}/check-runs?per_page=100",
            "-H",
            "Accept: application/vnd.github+json",
        ],
        runner,
    )
    runs = checks.get("check_runs") if isinstance(checks, Mapping) else None
    total = checks.get("total_count") if isinstance(checks, Mapping) else None
    if (
        not isinstance(runs, list)
        or type(total) is not int
        or total != len(runs)
        or not runs
    ):
        raise GitHubGateError("CI check-run evidence is hollow or incomplete")
    if any(
        not isinstance(run, Mapping)
        or run.get("head_sha") != head_sha
        or run.get("status") != "completed"
        or run.get("conclusion") != "success"
        for run in runs
    ):
        raise GitHubGateError("exact-head CI contains incomplete or non-green checks")

    status_pages: list[list[Any]] = []
    for page_number in range(1, 11):
        page = _json(
            [
                "gh",
                "api",
                f"repos/{REPOSITORY}/commits/{head_sha}/statuses?per_page=100&page={page_number}",
            ],
            runner,
        )
        if not isinstance(page, list) or len(page) > 100:
            raise GitHubGateError("commit-status evidence is malformed or incomplete")
        status_pages.append(page)
        if len(page) < 100:
            break
    else:
        raise GitHubGateError("commit-status evidence exceeded the pagination bound")
    statuses = [status for page in status_pages for status in page]
    latest_statuses: dict[str, tuple[tuple[datetime, int], str]] = {}
    for status in statuses:
        if not isinstance(status, Mapping) or status.get("sha") != head_sha:
            raise GitHubGateError("exact-head commit status is not green")
        context = status.get("context")
        updated_at = status.get("updated_at")
        status_id = status.get("id")
        state = status.get("state")
        if (
            not isinstance(context, str)
            or not context
            or context != context.strip()
            or not isinstance(updated_at, str)
            or not isinstance(status_id, int)
            or isinstance(status_id, bool)
            or status_id <= 0
            or not isinstance(state, str)
            or state not in {"error", "failure", "pending", "success"}
        ):
            raise GitHubGateError("commit-status evidence is malformed or incomplete")
        timestamp = _parse_rfc3339_utc(updated_at)
        order = (timestamp, status_id)
        previous = latest_statuses.get(context)
        if previous is None or order > previous[0]:
            latest_statuses[context] = (order, state)
        elif order == previous[0] and state != previous[1]:
            raise GitHubGateError("commit-status order is ambiguous")
    if any(state != "success" for _, state in latest_statuses.values()):
        raise GitHubGateError("exact-head commit status is not green")

    _validate_live_final_graphql_snapshot(pr_number, head_sha, runner=runner)

    return {
        "ci": {
            "head_sha": head_sha,
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


def _validate_live_final_graphql_snapshot(
    pr_number: int,
    head_sha: str,
    *,
    runner: Runner = _default_runner,
) -> None:
    query = """
query($owner:String!,$name:String!,$number:Int!){
  repository(owner:$owner,name:$name){
    issue(number:1240){number state labels(first:100){nodes{name} pageInfo{hasNextPage}}}
    pullRequest(number:$number){
      number state isDraft mergeable headRefName headRefOid reviewDecision author{login}
      reviewThreads(first:100){nodes{isResolved} pageInfo{hasNextPage}}
      reviews(last:100){nodes{databaseId state submittedAt author{login}} pageInfo{hasPreviousPage}}
    }
  }
}
""".strip()
    payload = _json(
        [
            "gh",
            "api",
            "graphql",
            "-f",
            f"query={query}",
            "-F",
            "owner=marmot-protocol",
            "-F",
            "name=mdk",
            "-F",
            f"number={pr_number}",
        ],
        runner,
    )
    try:
        repository = payload["data"]["repository"]
        issue = repository["issue"]
        pr = repository["pullRequest"]
    except (KeyError, TypeError) as exc:
        raise GitHubGateError("final GitHub snapshot is missing") from exc
    labels = issue.get("labels") if isinstance(issue, Mapping) else None
    label_nodes = labels.get("nodes") if isinstance(labels, Mapping) else None
    if (
        not isinstance(issue, Mapping)
        or issue.get("number") != 1240
        or issue.get("state") != "OPEN"
        or not isinstance(labels, Mapping)
        or labels.get("pageInfo", {}).get("hasNextPage")
        or not isinstance(label_nodes, list)
        or "pip-ok"
        not in {node.get("name") for node in label_nodes if isinstance(node, Mapping)}
    ):
        raise GitHubGateError("final issue authorization snapshot is invalid")
    author = pr.get("author") if isinstance(pr, Mapping) else None
    threads = pr.get("reviewThreads") if isinstance(pr, Mapping) else None
    thread_nodes = threads.get("nodes") if isinstance(threads, Mapping) else None
    reviews = pr.get("reviews") if isinstance(pr, Mapping) else None
    review_nodes = reviews.get("nodes") if isinstance(reviews, Mapping) else None
    if (
        not isinstance(pr, Mapping)
        or pr.get("number") != pr_number
        or pr.get("state") != "OPEN"
        or pr.get("isDraft") is not True
        or pr.get("headRefOid") != head_sha
        or pr.get("mergeable") != "MERGEABLE"
        or pr.get("reviewDecision") not in {None, "REVIEW_REQUIRED", "APPROVED"}
        or not isinstance(author, Mapping)
        or author.get("login") != EXPECTED_AUTHOR
        or not isinstance(pr.get("headRefName"), str)
        or not pr["headRefName"].startswith("pip/")
        or not isinstance(threads, Mapping)
        or threads.get("pageInfo", {}).get("hasNextPage")
        or not isinstance(thread_nodes, list)
        or not isinstance(reviews, Mapping)
        or reviews.get("pageInfo", {}).get("hasPreviousPage")
        or not isinstance(review_nodes, list)
        or any(
            not isinstance(thread, Mapping) or not thread.get("isResolved")
            for thread in thread_nodes
        )
    ):
        raise GitHubGateError("final PR authorization snapshot is invalid")
    _assert_no_effective_changes_request(review_nodes)


def validate_live_final_snapshot(
    pr_number: int,
    head_sha: str,
    *,
    runner: Runner = _default_runner,
) -> None:
    fetch_live_final_evidence(pr_number, head_sha, runner=runner)
