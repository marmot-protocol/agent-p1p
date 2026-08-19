from __future__ import annotations

import hashlib
import json
import os
import re
import stat
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from .case_store import CaseStore, CaseStoreError
from .kanban_router import canonical_route_id

CANARY_CASE_ID = "mdk#1240"
CANARY_PLANNER_COMMENT_ID = 5304234568
CANARY_PLANNER_COMMENT_SHA256 = (
    "c663222c82517f0978d8e66b64dc970799663e5c58fbd56cafe5d2a2e1681763"
)
LEGACY_MUTABLE_PLANNER_COMMENTS = {
    5331190946: "45f2b9aa8bd9efec869229b75e321343f7f882c308f3c8ba4c2bd3c125994e55"
}
TRUSTED_HUMAN_ACTOR = "erskingardner"
TRUSTED_HUMAN_ID = 202880
PLANNER_ACTOR = "agent-p1p"
PLANNER_ACTOR_ID = 292420120
APPROVE_COMMAND = "Pip: approve exact scope"
REJECT_COMMAND = "Pip: reject"
NARROW_PREFIX = "Pip: narrow scope — "
APPROVE_ALIASES = frozenset(
    {"approve", "approved", "@agent-p1p approve", "@agent-p1p approved"}
)
REJECT_ALIASES = frozenset(
    {"reject", "rejected", "@agent-p1p reject", "@agent-p1p rejected"}
)
GITHUB_COMMENTS_URL = (
    "https://api.github.com/repos/marmot-protocol/mdk/issues/1240/comments"
)
GITHUB_MASTER_URL = "https://api.github.com/repos/marmot-protocol/mdk/commits/master"
GITHUB_ISSUE_URL = "https://api.github.com/repos/marmot-protocol/mdk/issues/1240"
CANARY_PLANNED_BASE_SHA = "735c8ff256d33be282044d13abd8dd92b57d4ec8"
MAX_GITHUB_PAGES = 1
MAX_GITHUB_RESPONSE_BYTES = 1_048_576
MAX_GITHUB_CREDENTIAL_BYTES = 512
PLANNER_OUTCOMES = frozenset(
    {
        "PROCEED",
        "ALREADY_FIXED",
        "NOT_REPRODUCIBLE",
        "DUPLICATE",
        "ROOT_CAUSE_DIFFERENT_SCOPE",
        "CROSS_REPO_DEPENDENCY",
        "WAITING_FOR_ISSUE_CREATOR",
        "NEEDS_HUMAN_SCOPE_DECISION",
        "ABANDON",
        "BLOCKED_UNEXPECTED_MODEL",
    }
)
HUMAN_WAIT_OUTCOMES = frozenset(
    {
        "NEEDS_HUMAN_SCOPE_DECISION",
        "ROOT_CAUSE_DIFFERENT_SCOPE",
        "WAITING_FOR_ISSUE_CREATOR",
    }
)
_RFC3339_UTC = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z")
_FORBIDDEN_SCOPE = re.compile(
    r"\b(?:crypto(?:graphy|graphic)?|encrypt(?:ed|ion|ing)?|mls|cgka)\b|"
    r"\b(?:key|keys|credential|credentials|secret|secrets|token|tokens)\b|"
    r"\btrust(?:ed)?\b|"
    r"\b(?:authentication|authori[sz]ation)\b|"
    r"(?:\bpush\b.{0,80}\b(?:payload|context|metadata)\b|"
    r"\b(?:payload|context|metadata)\b.{0,80}\bpush\b)",
    re.IGNORECASE,
)
_CANARY_SCOPE_SIGNAL = re.compile(
    r"\b(?:notification|trigger|projection|event|ffi)\b", re.IGNORECASE
)


class DecisionError(RuntimeError):
    """Human-decision evidence is malformed, stale, or unauthorized."""


class ExternalDecisionError(DecisionError):
    """External lookup or runtime credential delivery failed transiently."""


class PinnedEvidenceError(DecisionError):
    """The immutable planner anchor changed or disappeared."""


class NarrowingEvidenceError(PinnedEvidenceError):
    """Previously accepted narrowing evidence changed or disappeared."""


def _parse_comment_timestamp(value: Any) -> datetime:
    if not isinstance(value, str) or _RFC3339_UTC.fullmatch(value) is None:
        raise DecisionError("GitHub comment timestamp is malformed")
    try:
        return datetime.fromisoformat(value.removesuffix("Z") + "+00:00")
    except ValueError as exc:
        raise DecisionError("GitHub comment timestamp is malformed") from exc


def _comment_order(comment: dict[str, Any], field: str) -> tuple[datetime, int]:
    return (_parse_comment_timestamp(comment[field]), comment["id"])


def _github_headers() -> dict[str, str]:
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "pip-v2-control/2",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    credentials_directory = os.environ.get("CREDENTIALS_DIRECTORY")
    if not credentials_directory:
        raise ExternalDecisionError("GitHub credential is unavailable")
    credential_path = Path(credentials_directory) / "github.token"
    descriptor = -1
    try:
        descriptor = os.open(
            credential_path,
            os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0),
        )
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_size > MAX_GITHUB_CREDENTIAL_BYTES
        ):
            raise ExternalDecisionError("GitHub credential is malformed")
        raw = os.read(descriptor, MAX_GITHUB_CREDENTIAL_BYTES + 1)
    except OSError as exc:
        raise ExternalDecisionError("GitHub credential is unavailable") from exc
    finally:
        if descriptor >= 0:
            os.close(descriptor)
    token = raw.removesuffix(b"\n")
    if (
        not token
        or b"\n" in token
        or b"\r" in token
        or not re.fullmatch(rb"[A-Za-z0-9_]+", token)
    ):
        raise ExternalDecisionError("GitHub credential is malformed")
    headers["Authorization"] = f"Bearer {token.decode('ascii')}"
    return headers


@dataclass(frozen=True)
class HumanDecision:
    kind: str
    actor: str
    comment_id: int
    comment_url: str
    body_sha256: str
    narrowed_scope: str | None


@dataclass(frozen=True)
class PlannerEvidence:
    comment_id: int
    comment_url: str
    body_sha256: str
    plan_version: int
    planned_base_sha: str
    updated_at: str
    narrowing_comment_id: int | None = None
    narrowing_body_sha256: str | None = None
    narrowed_scope: str | None = None
    outcome: str = "NEEDS_HUMAN_SCOPE_DECISION"
    execution_task_id: str | None = None
    authorized_scope: str | None = None


def fetch_canary_comments(
    *, opener: Callable[..., Any] = urlopen
) -> list[dict[str, Any]]:
    comments: list[dict[str, Any]] = []
    for page in range(1, MAX_GITHUB_PAGES + 1):
        request = Request(
            f"{GITHUB_COMMENTS_URL}?per_page=100&page={page}",
            headers=_github_headers(),
        )
        try:
            with opener(request, timeout=10) as response:
                raw = response.read(MAX_GITHUB_RESPONSE_BYTES + 1)
        except (HTTPError, URLError, TimeoutError, OSError) as exc:
            raise ExternalDecisionError("GitHub decision lookup failed") from exc
        if len(raw) > MAX_GITHUB_RESPONSE_BYTES:
            raise ExternalDecisionError("GitHub decision response is too large")
        try:
            payload = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise ExternalDecisionError(
                "GitHub decision response is malformed"
            ) from exc
        if not isinstance(payload, list):
            raise ExternalDecisionError(
                "GitHub decision response is not a comment list"
            )
        comments.extend(payload)
        if len(payload) < 100:
            return comments
    raise ExternalDecisionError("GitHub decision pagination limit exceeded")


def fetch_canary_base_sha(*, opener: Callable[..., Any] = urlopen) -> str:
    request = Request(
        GITHUB_MASTER_URL,
        headers=_github_headers(),
    )
    try:
        with opener(request, timeout=10) as response:
            raw = response.read(MAX_GITHUB_RESPONSE_BYTES + 1)
    except (HTTPError, URLError, TimeoutError, OSError) as exc:
        raise ExternalDecisionError("GitHub base lookup failed") from exc
    if len(raw) > MAX_GITHUB_RESPONSE_BYTES:
        raise ExternalDecisionError("GitHub base response is too large")
    try:
        payload = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ExternalDecisionError("GitHub base response is malformed") from exc
    sha = payload.get("sha") if isinstance(payload, dict) else None
    if not isinstance(sha, str) or re.fullmatch(r"[0-9a-f]{40}", sha) is None:
        raise ExternalDecisionError("GitHub base response has no valid commit SHA")
    return sha


def fetch_canary_issue_authorization(*, opener: Callable[..., Any] = urlopen) -> None:
    request = Request(
        GITHUB_ISSUE_URL,
        headers=_github_headers(),
    )
    try:
        with opener(request, timeout=10) as response:
            raw = response.read(MAX_GITHUB_RESPONSE_BYTES + 1)
    except (HTTPError, URLError, TimeoutError, OSError) as exc:
        raise ExternalDecisionError("GitHub issue authorization lookup failed") from exc
    if len(raw) > MAX_GITHUB_RESPONSE_BYTES:
        raise ExternalDecisionError("GitHub issue authorization response is too large")
    try:
        payload = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ExternalDecisionError(
            "GitHub issue authorization response is malformed"
        ) from exc
    labels = payload.get("labels") if isinstance(payload, dict) else None
    names = (
        {
            label.get("name")
            for label in labels
            if isinstance(label, dict) and isinstance(label.get("name"), str)
        }
        if isinstance(labels, list)
        else set()
    )
    if (
        not isinstance(payload, dict)
        or payload.get("number") != 1240
        or payload.get("state") != "open"
        or "pip-ok" not in names
    ):
        raise DecisionError("protected issue is closed or missing pip-ok")


def _validated_comment(raw: Any) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise DecisionError("GitHub comment response is malformed")
    user = raw.get("user")
    if (
        not isinstance(user, dict)
        or not isinstance(user.get("login"), str)
        or type(user.get("id")) is not int
    ):
        raise DecisionError("GitHub comment author is malformed")
    comment_id = raw.get("id")
    body = raw.get("body")
    created_at = raw.get("created_at")
    updated_at = raw.get("updated_at")
    url = raw.get("html_url")
    if (
        type(comment_id) is not int
        or comment_id < 1
        or not isinstance(body, str)
        or not isinstance(created_at, str)
        or not isinstance(updated_at, str)
        or not isinstance(url, str)
    ):
        raise DecisionError("GitHub comment fields are malformed")
    expected_url = (
        f"https://github.com/marmot-protocol/mdk/issues/1240#issuecomment-{comment_id}"
    )
    if url != expected_url:
        raise DecisionError("GitHub comment URL is outside the authorized issue")
    timestamps = [_parse_comment_timestamp(value) for value in (created_at, updated_at)]
    if timestamps[1] < timestamps[0]:
        raise DecisionError("GitHub comment timestamp order is invalid")
    return raw


def _planner_evidence(validated: list[dict[str, Any]]) -> PlannerEvidence:
    candidates = [
        comment
        for comment in validated
        if comment["user"]["login"] == PLANNER_ACTOR
        and comment["user"]["id"] == PLANNER_ACTOR_ID
        and (
            comment["id"] == CANARY_PLANNER_COMMENT_ID
            or comment["body"].startswith("Pip planning result for #1240 — plan v")
        )
    ]
    versioned: list[tuple[int, dict[str, Any]]] = []
    seen_versions: dict[int, int] = {}
    for candidate in candidates:
        candidate_header = re.search(
            r"^Pip planning result for #1240 — plan v(\d+) \(`([A-Z_]+)`\)",
            candidate["body"],
        )
        if candidate_header is None:
            if candidate["id"] == CANARY_PLANNER_COMMENT_ID:
                versioned.append((1, candidate))
                continue
            raise PinnedEvidenceError("planner comment heading is malformed")
        candidate_version = int(candidate_header.group(1))
        if candidate_version < 1:
            raise PinnedEvidenceError("planner version is invalid")
        candidate_digest = hashlib.sha256(candidate["body"].encode()).hexdigest()
        legacy_digest = LEGACY_MUTABLE_PLANNER_COMMENTS.get(candidate["id"])
        if candidate["id"] == CANARY_PLANNER_COMMENT_ID:
            legacy_digest = CANARY_PLANNER_COMMENT_SHA256
        if legacy_digest is not None and (candidate_digest != legacy_digest):
            raise PinnedEvidenceError("legacy planner comment digest does not match")
        if (
            candidate["id"] != CANARY_PLANNER_COMMENT_ID
            and legacy_digest is None
            and candidate["created_at"] != candidate["updated_at"]
        ):
            raise PinnedEvidenceError("immutable planner comment was edited")
        prior_id = seen_versions.get(candidate_version)
        if prior_id is not None and prior_id != candidate["id"]:
            raise PinnedEvidenceError("planner version has multiple comments")
        seen_versions[candidate_version] = candidate["id"]
        versioned.append((candidate_version, candidate))
    planner = max(
        versioned,
        key=lambda item: (item[0], _comment_order(item[1], "created_at")),
        default=None,
    )
    if planner is None:
        raise PinnedEvidenceError("pinned planner comment is missing")
    _, planner = planner
    digest = hashlib.sha256(planner["body"].encode()).hexdigest()
    header = re.search(
        r"^Pip planning result for #1240 — plan v(\d+) \(`([A-Z_]+)`\)",
        planner["body"],
    )
    base = re.search(r"current `master` at `([0-9a-f]{40})`", planner["body"])
    narrowing_comment_id = None
    narrowing_body_sha256 = None
    narrowed_scope = None
    bindings = re.findall(
        r"^Pip narrowing binding: (\{[^\n]+\})$",
        planner["body"],
        re.MULTILINE,
    )
    if len(bindings) > 1:
        raise PinnedEvidenceError("planner has multiple narrowing bindings")
    if bindings:
        try:
            bound = json.loads(bindings[0])
        except json.JSONDecodeError as exc:
            raise PinnedEvidenceError("planner narrowing binding is malformed") from exc
        if (
            not isinstance(bound, dict)
            or set(bound) != {"comment_id", "body_sha256", "narrowed_scope"}
            or type(bound["comment_id"]) is not int
            or bound["comment_id"] < 1
            or not isinstance(bound["body_sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", bound["body_sha256"]) is None
            or not isinstance(bound["narrowed_scope"], str)
            or not bound["narrowed_scope"]
            or len(bound["narrowed_scope"]) > 1_000
            or "\n" in bound["narrowed_scope"]
        ):
            raise PinnedEvidenceError("planner narrowing binding is invalid")
        narrowing_comment_id = bound["comment_id"]
        narrowing_body_sha256 = bound["body_sha256"]
        narrowed_scope = bound["narrowed_scope"]
    if header is not None and base is not None:
        version = int(header.group(1))
        outcome = header.group(2)
        if version < 1 or outcome not in PLANNER_OUTCOMES:
            raise PinnedEvidenceError("planner version or outcome is invalid")
        execution_task_id = None
        authorized_scope = None
        execution_bindings = re.findall(
            r"^Pip execution binding: (\{[^\n]+\})$",
            planner["body"],
            re.MULTILINE,
        )
        if outcome == "PROCEED":
            if len(execution_bindings) != 1:
                raise PinnedEvidenceError(
                    "PROCEED planner result requires one execution binding"
                )
            try:
                execution = json.loads(execution_bindings[0])
            except json.JSONDecodeError as exc:
                raise PinnedEvidenceError(
                    "planner execution binding is malformed"
                ) from exc
            if (
                not isinstance(execution, dict)
                or set(execution)
                != {
                    "authorized_scope",
                    "dependencies",
                    "open_decisions",
                    "outcome",
                    "plan_version",
                    "sensitive_scope",
                    "task_id",
                }
                or not isinstance(execution["authorized_scope"], str)
                or not execution["authorized_scope"]
                or len(execution["authorized_scope"]) > 1_000
                or "\n" in execution["authorized_scope"]
                or _FORBIDDEN_SCOPE.search(execution["authorized_scope"]) is not None
                or _CANARY_SCOPE_SIGNAL.search(execution["authorized_scope"]) is None
                or (
                    narrowed_scope is not None
                    and execution["authorized_scope"] != narrowed_scope
                )
                or execution["dependencies"] != []
                or execution["open_decisions"] != []
                or execution["sensitive_scope"] != []
                or execution["outcome"] != outcome
                or execution["plan_version"] != version
                or not isinstance(execution["task_id"], str)
                or re.fullmatch(r"t_[A-Za-z0-9_]+", execution["task_id"]) is None
                or json.dumps(execution, separators=(",", ":"), sort_keys=True)
                != execution_bindings[0]
            ):
                raise PinnedEvidenceError("planner execution binding is invalid")
            execution_task_id = execution["task_id"]
            authorized_scope = execution["authorized_scope"]
        elif execution_bindings:
            raise PinnedEvidenceError(
                "non-PROCEED planner result cannot carry an execution binding"
            )
        return PlannerEvidence(
            comment_id=planner["id"],
            comment_url=planner["html_url"],
            body_sha256=digest,
            plan_version=version,
            planned_base_sha=base.group(1),
            updated_at=planner["updated_at"],
            narrowing_comment_id=narrowing_comment_id,
            narrowing_body_sha256=narrowing_body_sha256,
            narrowed_scope=narrowed_scope,
            outcome=outcome,
            execution_task_id=execution_task_id,
            authorized_scope=authorized_scope,
        )
    if (
        planner["id"] != CANARY_PLANNER_COMMENT_ID
        or digest != CANARY_PLANNER_COMMENT_SHA256
    ):
        raise PinnedEvidenceError("pinned planner comment digest does not match")
    return PlannerEvidence(
        comment_id=planner["id"],
        comment_url=planner["html_url"],
        body_sha256=digest,
        plan_version=1,
        planned_base_sha=CANARY_PLANNED_BASE_SHA,
        updated_at=planner["updated_at"],
    )


def parse_planner_evidence(comments: list[dict[str, Any]]) -> PlannerEvidence:
    return _planner_evidence([_validated_comment(comment) for comment in comments])


def _classify_human_body(raw_body: str) -> tuple[str, str | None]:
    body = raw_body.strip()
    alias_body = raw_body.removesuffix("\n")
    alias = (
        alias_body.strip().casefold()
        if "\n" not in alias_body and "\r" not in alias_body
        else ""
    )
    if body == APPROVE_COMMAND or alias in APPROVE_ALIASES:
        return "approve", None
    if body == REJECT_COMMAND or alias in REJECT_ALIASES:
        return "reject", None
    if body.startswith(NARROW_PREFIX):
        candidate = body[len(NARROW_PREFIX) :].strip()
        contains_credential = re.search(
            r"gh[pousr]_[A-Za-z0-9]{20,}|"
            r"nsec1[023456789acdefghjklmnpqrstuvwxyz]{20,}|"
            r"sk-(?:proj-)?[A-Za-z0-9_-]{20,}|"
            r"AKIA[0-9A-Z]{16}|"
            r"xox[baprs]-[0-9A-Za-z-]{20,}",
            candidate,
        )
        if (
            candidate
            and len(candidate) <= 1_000
            and "\n" not in candidate
            and contains_credential is None
        ):
            return "narrow", candidate
    return "unrecognized", None


def parse_human_decision(comments: list[dict[str, Any]]) -> HumanDecision | None:
    validated = [_validated_comment(comment) for comment in comments]
    planner = _planner_evidence(validated)
    trusted_all = [
        comment
        for comment in validated
        if comment["user"]["login"] == TRUSTED_HUMAN_ACTOR
        and comment["user"]["id"] == TRUSTED_HUMAN_ID
    ]
    trusted_after = [
        comment
        for comment in trusted_all
        if _parse_comment_timestamp(comment["created_at"])
        > _parse_comment_timestamp(planner.updated_at)
    ]
    latest_after = max(
        trusted_after,
        key=lambda comment: _comment_order(comment, "created_at"),
        default=None,
    )

    classified = [
        (comment, *_classify_human_body(comment["body"])) for comment in trusted_all
    ]
    latest_reject = max(
        (item for item in classified if item[1] == "reject"),
        key=lambda item: _comment_order(item[0], "created_at"),
        default=None,
    )
    latest_approve_after = max(
        (
            item
            for item in classified
            if item[1] == "approve"
            and _parse_comment_timestamp(item[0]["created_at"])
            > _parse_comment_timestamp(planner.updated_at)
        ),
        key=lambda item: _comment_order(item[0], "created_at"),
        default=None,
    )
    if (
        latest_reject is not None
        and latest_approve_after is not None
        and _comment_order(latest_approve_after[0], "created_at")
        > _comment_order(latest_reject[0], "created_at")
    ):
        latest_reject = None

    latest_narrow = max(
        (item for item in classified if item[1] == "narrow"),
        key=lambda item: _comment_order(item[0], "created_at"),
        default=None,
    )
    if latest_narrow is not None:
        narrow_digest = hashlib.sha256(latest_narrow[0]["body"].encode()).hexdigest()
        if (
            planner.narrowing_comment_id == latest_narrow[0]["id"]
            and planner.narrowing_body_sha256 == narrow_digest
            and planner.narrowed_scope == latest_narrow[2]
        ):
            latest_narrow = None

    persistent = max(
        (item for item in (latest_reject, latest_narrow) if item is not None),
        key=lambda item: _comment_order(item[0], "created_at"),
        default=None,
    )
    latest = persistent[0] if persistent is not None else latest_after
    if latest is None:
        return None
    kind, narrowed_scope = _classify_human_body(latest["body"])
    digest = hashlib.sha256(latest["body"].encode()).hexdigest()
    return HumanDecision(
        kind=kind,
        actor=latest["user"]["login"],
        comment_id=latest["id"],
        comment_url=latest["html_url"],
        body_sha256=digest,
        narrowed_scope=narrowed_scope,
    )


def _decision_marker(decision: HumanDecision, planner: PlannerEvidence) -> str:
    return (
        f"HUMAN_DECISION actor={decision.actor} comment_id={decision.comment_id} "
        f"body_sha256={decision.body_sha256} url={decision.comment_url} "
        f"planner_comment_id={planner.comment_id} planner_sha256={planner.body_sha256} "
        f"plan_version={planner.plan_version} planned_base_sha={planner.planned_base_sha}"
    )


def _observed_human_evidence(events: list[dict[str, Any]]) -> dict[int, set[str]]:
    observed: dict[int, set[str]] = {}
    for event in events:
        match = re.search(
            r"HUMAN_DECISION actor=erskingardner comment_id=(\d+) "
            r"body_sha256=([0-9a-f]{64})",
            event["reason"],
        )
        if match is not None:
            observed.setdefault(int(match.group(1)), set()).add(match.group(2))
    return observed


def _latest_narrowing(
    events: list[dict[str, Any]], comments: list[dict[str, Any]]
) -> HumanDecision | None:
    marker = None
    for event in events:
        if "HUMAN_NARROWED_SCOPE" not in event["reason"]:
            continue
        match = re.search(
            r"HUMAN_DECISION actor=erskingardner comment_id=(\d+) "
            r"body_sha256=([0-9a-f]{64})",
            event["reason"],
        )
        if match is not None:
            marker = (int(match.group(1)), match.group(2))
    if marker is None:
        return None
    for raw in comments:
        comment = _validated_comment(raw)
        if comment["id"] != marker[0]:
            continue
        digest = hashlib.sha256(comment["body"].encode()).hexdigest()
        if (
            digest != marker[1]
            or comment["user"]["login"] != TRUSTED_HUMAN_ACTOR
            or comment["user"]["id"] != TRUSTED_HUMAN_ID
        ):
            raise NarrowingEvidenceError("narrowing evidence changed")
        body = comment["body"].strip()
        if not body.startswith(NARROW_PREFIX):
            raise NarrowingEvidenceError("narrowing evidence is no longer valid")
        scope = body[len(NARROW_PREFIX) :].strip()
        if not scope:
            raise NarrowingEvidenceError("narrowing evidence has empty scope")
        return HumanDecision(
            kind="narrow",
            actor=TRUSTED_HUMAN_ACTOR,
            comment_id=comment["id"],
            comment_url=comment["html_url"],
            body_sha256=digest,
            narrowed_scope=scope,
        )
    raise NarrowingEvidenceError("narrowing evidence disappeared")


def _active_route(result: dict[str, Any], action: str, state: str) -> dict[str, Any]:
    payload = {
        "ok": True,
        "case_id": CANARY_CASE_ID,
        **result,
        "action": action,
        "state": state,
    }
    payload.setdefault("narrowed_scope", None)
    payload["route_id"] = canonical_route_id(payload)
    return payload


def _active_automatic_planner(
    events: list[dict[str, Any]], comments: list[dict[str, Any]]
) -> PlannerEvidence:
    marker = None
    pattern = re.compile(
        r"AUTOMATIC_PLAN planner_comment_id=(\d+) "
        r"planner_sha256=([0-9a-f]{64}) plan_version=(\d+) "
        r"planned_base_sha=([0-9a-f]{40})"
    )
    for event in events:
        match = pattern.search(event["reason"])
        if match is not None:
            marker = match
    if marker is None:
        raise PinnedEvidenceError("active automatic planner marker is missing")
    comment_id = int(marker.group(1))
    matching = [
        comment
        for comment in comments
        if isinstance(comment, dict) and comment.get("id") == comment_id
    ]
    if len(matching) != 1:
        raise PinnedEvidenceError("active automatic planner comment is missing")
    planner = parse_planner_evidence(matching)
    if (
        planner.body_sha256 != marker.group(2)
        or planner.plan_version != int(marker.group(3))
        or planner.planned_base_sha != marker.group(4)
        or planner.outcome != "PROCEED"
    ):
        raise PinnedEvidenceError("active automatic planner evidence changed")
    return planner


def _history_regression_result(
    store: CaseStore, case: dict[str, Any]
) -> dict[str, Any]:
    state = case["state"]
    if state in {"BUILDING", "REVIEWING", "FINAL_REVIEW", "SHADOW_READY"}:
        state = store.set_state(
            CANARY_CASE_ID,
            state,
            "BLOCKED",
            "HUMAN_DECISION_HISTORY_REGRESSED",
        ).value
    return {
        "action": "stop",
        "decision": None,
        "state": state,
        "error": "human_decision_history_regressed",
    }


def _automatic_plan_result(
    store: CaseStore,
    case: dict[str, Any],
    events: list[dict[str, Any]],
    planner: PlannerEvidence,
) -> dict[str, Any]:
    if planner.execution_task_id is None:
        raise PinnedEvidenceError("automatic plan has no execution binding")
    marker = (
        f"AUTOMATIC_PLAN planner_comment_id={planner.comment_id} "
        f"planner_sha256={planner.body_sha256} plan_version={planner.plan_version} "
        f"planned_base_sha={planner.planned_base_sha}"
    )
    current_state = case["state"]
    exact_marker_seen = any(marker in event["reason"] for event in events)
    any_automatic_seen = any(
        "AUTOMATIC_PLAN planner_comment_id=" in event["reason"] for event in events
    )
    if current_state == "BUILDING" and any_automatic_seen and not exact_marker_seen:
        raise PinnedEvidenceError(
            "active automatic work must retain its original planner evidence"
        )
    if current_state == "PLANNING":
        current_plan_version = case["plan_version"]
        if current_plan_version > planner.plan_version:
            raise DecisionError("planner evidence regressed to an older version")
        for expected in range(current_plan_version, planner.plan_version):
            store.record_plan_version(CANARY_CASE_ID, expected, expected + 1)
        if not exact_marker_seen:
            current_state = store.set_state(
                CANARY_CASE_ID,
                "PLANNING",
                "PROCEED",
                marker,
            ).value
        else:
            current_state = store.get_case(CANARY_CASE_ID)["state"]
    if current_state != "BUILDING":
        raise DecisionError(
            f"automatic plan cannot be applied from state {current_state}"
        )
    result = {
        "decision": "automatic",
        "comment_id": planner.comment_id,
        "comment_url": planner.comment_url,
        "evidence_body_sha256": planner.body_sha256,
        "repository": "marmot-protocol/mdk",
        "issue_number": 1240,
        "issue_url": "https://github.com/marmot-protocol/mdk/issues/1240",
        "planner_comment_id": planner.comment_id,
        "planner_comment_url": planner.comment_url,
        "planner_body_sha256": planner.body_sha256,
        "plan_version": planner.plan_version,
        "planned_base_sha": planner.planned_base_sha,
        "planner_outcome": planner.outcome,
        "planner_task_id": planner.execution_task_id,
        "authorized_scope": planner.authorized_scope,
    }
    if planner.narrowed_scope is not None:
        result["narrowed_scope"] = planner.narrowed_scope
    return _active_route(result, "dispatch_builder", current_state)


def reconcile_human_decision(
    store: CaseStore,
    comments: list[dict[str, Any]],
    *,
    current_base_sha: str = CANARY_PLANNED_BASE_SHA,
) -> dict[str, Any]:
    if re.fullmatch(r"[0-9a-f]{40}", current_base_sha) is None:
        raise DecisionError("current base SHA is invalid")
    case = store.get_case(CANARY_CASE_ID)
    events = store.list_events(CANARY_CASE_ID)
    has_accepted_decision = any(
        "HUMAN_DECISION actor=" in event["reason"] for event in events
    )
    has_automatic_plan = any(
        "AUTOMATIC_PLAN planner_comment_id=" in event["reason"] for event in events
    )
    observed_evidence = _observed_human_evidence(events)
    try:
        planner = parse_planner_evidence(comments)
        decision = parse_human_decision(comments)
        prior_narrowing = _latest_narrowing(events, comments)
    except NarrowingEvidenceError:
        if has_accepted_decision or has_automatic_plan:
            return _history_regression_result(store, case)
        raise
    except PinnedEvidenceError:
        if has_accepted_decision or has_automatic_plan:
            result = _history_regression_result(store, case)
            result["error"] = "pinned_planner_evidence_invalid"
            return result
        raise
    if (
        has_automatic_plan
        and case["state"] == "BUILDING"
        and (decision is None or decision.kind in {"approve", "unrecognized"})
    ):
        try:
            active_planner = _active_automatic_planner(events, comments)
        except PinnedEvidenceError:
            result = _history_regression_result(store, case)
            result["error"] = "active_planner_evidence_invalid"
            return result
        return _automatic_plan_result(store, case, events, active_planner)
    if planner.outcome == "PROCEED" and (
        decision is None or decision.kind in {"approve", "unrecognized"}
    ):
        return _automatic_plan_result(store, case, events, planner)
    if has_automatic_plan and case["state"] == "BUILDING" and decision is None:
        state = store.set_state(
            CANARY_CASE_ID,
            "BUILDING",
            "BLOCKED",
            f"AUTOMATIC_PLAN_SUPERSEDED planner_comment_id={planner.comment_id} "
            f"planner_sha256={planner.body_sha256} outcome={planner.outcome}",
        )
        return {
            "action": "stop",
            "decision": None,
            "state": state.value,
            "error": "planner_requires_human_decision",
        }
    if (
        decision is not None
        and observed_evidence
        and decision.comment_id < max(observed_evidence)
    ):
        return _history_regression_result(store, case)
    if decision is None:
        if observed_evidence:
            return _history_regression_result(store, case)
        if case["state"] == "BUILDING" and has_accepted_decision:
            state = store.set_state(
                CANARY_CASE_ID,
                "BUILDING",
                "BLOCKED",
                "HUMAN_DECISION_EVIDENCE_MISSING",
            )
            return {"action": "stop", "decision": None, "state": state.value}
        return {"action": "wait", "decision": None, "state": case["state"]}
    if (
        decision.comment_id in observed_evidence
        and decision.body_sha256 not in observed_evidence[decision.comment_id]
    ):
        mutated = _history_regression_result(store, case)
        mutated["error"] = "human_decision_evidence_mutated"
        return mutated
    result = {
        "decision": decision.kind,
        "comment_id": decision.comment_id,
        "comment_url": decision.comment_url,
        "evidence_body_sha256": decision.body_sha256,
        "repository": "marmot-protocol/mdk",
        "issue_number": 1240,
        "issue_url": "https://github.com/marmot-protocol/mdk/issues/1240",
        "planner_comment_id": planner.comment_id,
        "planner_comment_url": planner.comment_url,
        "planner_body_sha256": planner.body_sha256,
        "plan_version": planner.plan_version,
        "planned_base_sha": planner.planned_base_sha,
    }
    effective_narrowing = decision.narrowed_scope
    if effective_narrowing is None and prior_narrowing is not None:
        effective_narrowing = prior_narrowing.narrowed_scope
    if effective_narrowing is not None:
        result["narrowed_scope"] = effective_narrowing
    if decision.kind == "unrecognized":
        if (
            case["state"]
            in {
                "BUILDING",
                "REVIEWING",
                "FINAL_REVIEW",
                "SHADOW_READY",
            }
            and has_accepted_decision
        ):
            invalidated = _history_regression_result(store, case)
            invalidated["error"] = "human_decision_invalidated"
            return {**result, **invalidated}
        return {**result, "action": "request_explicit_command", "state": case["state"]}

    if (
        decision.kind == "narrow"
        and prior_narrowing is not None
        and decision.comment_id == prior_narrowing.comment_id
        and decision.body_sha256 == prior_narrowing.body_sha256
    ):
        return {
            **result,
            "action": "continue",
            "state": case["state"],
            "reason": "narrowed_plan_not_replaced",
        }

    marker = _decision_marker(decision, planner)
    unresolved_narrow = prior_narrowing is not None and (
        planner.narrowing_comment_id != prior_narrowing.comment_id
        or planner.narrowing_body_sha256 != prior_narrowing.body_sha256
        or planner.narrowed_scope != prior_narrowing.narrowed_scope
    )
    if decision.kind == "approve" and unresolved_narrow:
        return {
            **result,
            "action": "continue",
            "state": case["state"],
            "reason": "narrowed_plan_not_replaced",
        }
    prior = [event for event in events if marker in event["reason"]]
    if prior:
        current_state = store.get_case(CANARY_CASE_ID)["state"]
        if decision.kind == "approve":
            if planner.outcome not in (HUMAN_WAIT_OUTCOMES | {"PROCEED"}):
                action = "wait"
            elif current_state == "PLANNING":
                current_state = store.set_state(
                    CANARY_CASE_ID,
                    "PLANNING",
                    "PROCEED",
                    f"HUMAN_APPROVED_SCOPE_REUSED {marker}",
                ).value
            action = "dispatch_builder" if current_state == "BUILDING" else "continue"
        elif decision.kind == "narrow":
            action = "replan" if current_state == "PLANNING" else "continue"
        else:
            action = "stop"
        if action in {"dispatch_builder", "replan"}:
            return _active_route(result, action, current_state)
        return {**result, "action": action, "state": current_state}
    if decision.kind == "approve" and planner.outcome not in (
        HUMAN_WAIT_OUTCOMES | {"PROCEED"}
    ):
        return {
            **result,
            "action": "wait",
            "state": case["state"],
            "reason": "planner_outcome_not_executable",
        }
    try:
        if decision.kind == "approve":
            if case["state"] == "PLANNING":
                current_plan_version = case["plan_version"]
                if current_plan_version > planner.plan_version:
                    raise DecisionError(
                        "planner evidence regressed to an older version"
                    )
                for expected in range(current_plan_version, planner.plan_version):
                    store.record_plan_version(CANARY_CASE_ID, expected, expected + 1)
                state = store.set_state(
                    CANARY_CASE_ID,
                    "PLANNING",
                    "PROCEED",
                    f"HUMAN_APPROVED_SCOPE {marker}",
                )
                action = "dispatch_builder"
            elif case["state"] == "BUILDING":
                state = store.set_state(
                    CANARY_CASE_ID,
                    "BUILDING",
                    "HUMAN_REAFFIRMED_SCOPE",
                    marker,
                )
                action = "dispatch_builder"
            else:
                raise DecisionError(
                    f"approval cannot be applied from state {case['state']}"
                )
        elif decision.kind == "narrow":
            if case["state"] == "PLANNING":
                outcome = "HUMAN_NARROWED_SCOPE"
            elif case["state"] == "BUILDING":
                outcome = "RETURN_TO_PLANNING"
            else:
                raise DecisionError(
                    f"narrowed scope cannot be applied from state {case['state']}"
                )
            state = store.set_state(
                CANARY_CASE_ID,
                case["state"],
                outcome,
                f"HUMAN_NARROWED_SCOPE {marker}",
            )
            action = "replan"
        else:
            if case["state"] not in {"PLANNING", "BUILDING"}:
                raise DecisionError(
                    f"rejection cannot be applied from state {case['state']}"
                )
            state = store.set_state(
                CANARY_CASE_ID,
                case["state"],
                "ABANDON",
                f"HUMAN_REJECTED_SCOPE {marker}",
            )
            action = "stop"
    except CaseStoreError as exc:
        raise DecisionError(str(exc)) from exc
    if action in {"dispatch_builder", "replan"}:
        return _active_route(result, action, state.value)
    return {**result, "action": action, "state": state.value}
