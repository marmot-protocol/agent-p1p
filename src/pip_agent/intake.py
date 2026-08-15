from __future__ import annotations

import argparse
import json
import subprocess
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from .case_store import CaseStore


class IntakeError(RuntimeError):
    """An issue is not authorized for Pip v2 intake."""


@dataclass(frozen=True)
class IntakeCandidate:
    case_id: str
    repository: str
    issue_number: int
    title: str
    url: str
    authorization_actor: str
    intake_label: str


def authorize_issue(
    issue: Mapping[str, Any],
    timeline: Sequence[Mapping[str, Any]],
    config: Mapping[str, Any],
) -> IntakeCandidate:
    repository = config.get("repository")
    label = config.get("intake_label")
    trusted = set(config.get("trusted_issue_actors", []))
    excluded = set(config.get("excluded_issue_numbers", []))
    number = issue.get("number")
    if not isinstance(repository, str) or not repository:
        raise IntakeError("repository configuration is invalid")
    if label != "pip-ok":
        raise IntakeError("Pip v2 intake label must be pip-ok")
    if type(number) is not int or number < 1:
        raise IntakeError("issue number is invalid")
    if number in excluded:
        raise IntakeError(f"issue {number} is explicitly excluded")
    if "pull_request" in issue:
        raise IntakeError(f"{number} is a pull request, not an issue")
    if issue.get("state") != "open":
        raise IntakeError(f"issue {number} is not open")
    labels = {
        item.get("name") if isinstance(item, Mapping) else item
        for item in issue.get("labels", [])
    }
    if label not in labels:
        raise IntakeError(f"issue {number} does not have {label}")

    latest_label_event: Mapping[str, Any] | None = None
    for event in timeline:
        event_label = event.get("label")
        if (
            event.get("event") in {"labeled", "unlabeled"}
            and isinstance(event_label, Mapping)
            and event_label.get("name") == label
        ):
            latest_label_event = event
    if latest_label_event is None or latest_label_event.get("event") != "labeled":
        raise IntakeError(f"current {label} authorization event is missing")
    event_actor = latest_label_event.get("actor")
    authorization_actor = (
        event_actor.get("login") if isinstance(event_actor, Mapping) else None
    )
    if not isinstance(authorization_actor, str) or authorization_actor not in trusted:
        raise IntakeError(f"{label} was not applied by a trusted actor")
    title = issue.get("title")
    url = issue.get("html_url")
    if not isinstance(title, str) or not title:
        raise IntakeError("issue title is missing")
    expected_url = f"https://github.com/{repository}/issues/{number}"
    if not isinstance(url, str) or url != expected_url:
        raise IntakeError("issue URL is invalid")
    short_repo = repository.rsplit("/", 1)[-1]
    return IntakeCandidate(
        case_id=f"{short_repo}#{number}",
        repository=repository,
        issue_number=number,
        title=title,
        url=url,
        authorization_actor=authorization_actor,
        intake_label=label,
    )


def planner_task_command(
    candidate: IntakeCandidate, config: Mapping[str, Any]
) -> list[str]:
    if config.get("new_intake_enabled") is not True:
        raise IntakeError("new Pip v2 intake is disabled")
    if config.get("dispatch_enabled") is not True:
        raise IntakeError("Pip v2 dispatch is disabled")
    canary_issue = config.get("canary_issue_number")
    if type(canary_issue) is not int or canary_issue != candidate.issue_number:
        raise IntakeError("dispatch requires an exact one-issue canary binding")
    if (
        config.get("merge_mode") != "shadow"
        or config.get("autonomous_merge") is not False
    ):
        raise IntakeError("pilot must remain shadow and human-merge-only")
    board = config.get("board")
    if not isinstance(board, str) or not board:
        raise IntakeError("board configuration is invalid")
    model = config.get("models", {}).get("planner")
    if model != "openai-codex/gpt-5.6-sol":
        raise IntakeError("planner model configuration is invalid")
    body = json.dumps(
        {
            "workflow_version": 2,
            "case_id": candidate.case_id,
            "repository": candidate.repository,
            "issue_number": candidate.issue_number,
            "issue_url": candidate.url,
            "authorization_actor": candidate.authorization_actor,
            "intake_label": candidate.intake_label,
            "required_outcome_schema": "planner-result",
            "merge_mode": "shadow",
        },
        sort_keys=True,
    )
    return [
        "hermes",
        "kanban",
        "--board",
        board,
        "create",
        f"Plan {candidate.case_id}: {candidate.title}",
        "--body",
        body,
        "--assignee",
        "planner",
        "--workspace",
        "scratch",
        "--idempotency-key",
        f"pip-v2:{candidate.repository}:{candidate.issue_number}:plan-v1",
        "--created-by",
        "pip-v2-intake",
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
        "--json",
    ]


def _gh_json(endpoint: str, *, paginate: bool = False) -> Any:
    def fetch_page(page_endpoint: str) -> Any:
        command = [
            "gh",
            "api",
            page_endpoint,
            "-H",
            "Accept: application/vnd.github+json",
        ]
        completed = subprocess.run(
            command,
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode != 0:
            raise IntakeError(f"GitHub lookup failed: {completed.stderr.strip()}")
        try:
            return json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            raise IntakeError("GitHub returned malformed JSON") from exc

    if not paginate:
        return fetch_page(endpoint)

    if "per_page=" not in endpoint:
        separator = "&" if "?" in endpoint else "?"
        endpoint = f"{endpoint}{separator}per_page=100"
    separator = "&" if "?" in endpoint else "?"
    items: list[Any] = []
    for page in range(1, 101):
        payload = fetch_page(f"{endpoint}{separator}page={page}")
        if not isinstance(payload, list):
            raise IntakeError("GitHub pagination returned an invalid page")
        items.extend(payload)
        if len(payload) < 100:
            return items
    raise IntakeError("GitHub pagination exceeded the 100-page safety bound")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Authorize one Pip v2 GitHub issue")
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--issue", type=int, required=True)
    parser.add_argument("--database", type=Path)
    parser.add_argument("--enqueue", action="store_true")
    args = parser.parse_args(argv)
    config = json.loads(args.config.read_text())
    repository = config["repository"]
    issue = _gh_json(f"repos/{repository}/issues/{args.issue}")
    timeline = _gh_json(
        f"repos/{repository}/issues/{args.issue}/timeline?per_page=100",
        paginate=True,
    )
    candidate = authorize_issue(issue, timeline, config)
    command = planner_task_command(candidate, config) if args.enqueue else None
    if args.enqueue:
        if args.database is None:
            raise IntakeError("--database is required with --enqueue")
        assert command is not None
        store = CaseStore(args.database)
        store.ensure_case(
            candidate.case_id,
            candidate.repository,
            candidate.issue_number,
            candidate.intake_label,
        )
        completed = subprocess.run(command, text=True, capture_output=True, check=False)
        if completed.returncode != 0:
            raise IntakeError(
                f"planner task creation failed: {completed.stderr.strip()}"
            )
        task = json.loads(completed.stdout)
    else:
        task = None
    print(
        json.dumps(
            {
                "candidate": asdict(candidate),
                "enqueue_requested": args.enqueue,
                "task": task,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
