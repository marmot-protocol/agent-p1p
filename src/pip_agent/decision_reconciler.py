from __future__ import annotations

import hashlib
import json
import os
import re
import stat
from collections.abc import Callable
from dataclasses import dataclass
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


class DecisionError(RuntimeError):
    """Human-decision evidence is malformed, stale, or unauthorized."""


class PinnedEvidenceError(DecisionError):
    """The immutable planner anchor changed or disappeared."""


class NarrowingEvidenceError(PinnedEvidenceError):
    """Previously accepted narrowing evidence changed or disappeared."""


def _github_headers() -> dict[str, str]:
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "pip-v2-control/2",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    credentials_directory = os.environ.get("CREDENTIALS_DIRECTORY")
    if not credentials_directory:
        raise DecisionError("GitHub credential is unavailable")
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
            raise DecisionError("GitHub credential is malformed")
        raw = os.read(descriptor, MAX_GITHUB_CREDENTIAL_BYTES + 1)
    except OSError as exc:
        raise DecisionError("GitHub credential is unavailable") from exc
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
        raise DecisionError("GitHub credential is malformed")
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
            raise DecisionError("GitHub decision lookup failed") from exc
        if len(raw) > MAX_GITHUB_RESPONSE_BYTES:
            raise DecisionError("GitHub decision response is too large")
        try:
            payload = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecisionError("GitHub decision response is malformed") from exc
        if not isinstance(payload, list):
            raise DecisionError("GitHub decision response is not a comment list")
        comments.extend(payload)
        if len(payload) < 100:
            return comments
    raise DecisionError("GitHub decision pagination limit exceeded")


def fetch_canary_base_sha(*, opener: Callable[..., Any] = urlopen) -> str:
    request = Request(
        GITHUB_MASTER_URL,
        headers=_github_headers(),
    )
    try:
        with opener(request, timeout=10) as response:
            raw = response.read(MAX_GITHUB_RESPONSE_BYTES + 1)
    except (HTTPError, URLError, TimeoutError, OSError) as exc:
        raise DecisionError("GitHub base lookup failed") from exc
    if len(raw) > MAX_GITHUB_RESPONSE_BYTES:
        raise DecisionError("GitHub base response is too large")
    try:
        payload = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DecisionError("GitHub base response is malformed") from exc
    sha = payload.get("sha") if isinstance(payload, dict) else None
    if not isinstance(sha, str) or re.fullmatch(r"[0-9a-f]{40}", sha) is None:
        raise DecisionError("GitHub base response has no valid commit SHA")
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
        raise DecisionError("GitHub issue authorization lookup failed") from exc
    if len(raw) > MAX_GITHUB_RESPONSE_BYTES:
        raise DecisionError("GitHub issue authorization response is too large")
    try:
        payload = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DecisionError("GitHub issue authorization response is malformed") from exc
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
    planner = max(
        candidates,
        key=lambda item: (item["updated_at"], item["id"]),
        default=None,
    )
    if planner is None:
        raise PinnedEvidenceError("pinned planner comment is missing")
    digest = hashlib.sha256(planner["body"].encode()).hexdigest()
    header = re.search(
        r"^Pip planning result for #1240 — plan v(\d+) ", planner["body"]
    )
    base = re.search(r"current `master` at `([0-9a-f]{40})`", planner["body"])
    narrowing_comment_id = None
    narrowing_body_sha256 = None
    narrowed_scope = None
    binding = re.search(
        r"^Pip narrowing binding: (\{[^\n]+\})$",
        planner["body"],
        re.MULTILINE,
    )
    if binding is not None:
        try:
            bound = json.loads(binding.group(1))
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
        if version < 1:
            raise PinnedEvidenceError("planner version is invalid")
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


def parse_human_decision(comments: list[dict[str, Any]]) -> HumanDecision | None:
    validated = [_validated_comment(comment) for comment in comments]
    planner = _planner_evidence(validated)

    trusted = [
        comment
        for comment in validated
        if comment["created_at"] > planner.updated_at
        and comment["user"]["login"] == TRUSTED_HUMAN_ACTOR
        and comment["user"]["id"] == TRUSTED_HUMAN_ID
    ]
    if not trusted:
        return None
    latest = max(trusted, key=lambda comment: (comment["created_at"], comment["id"]))
    raw_body = latest["body"]
    body = raw_body.strip()
    alias = body.casefold() if "\n" not in raw_body and "\r" not in raw_body else ""
    kind = "unrecognized"
    narrowed_scope: str | None = None
    if body == APPROVE_COMMAND or alias in APPROVE_ALIASES:
        kind = "approve"
    elif body == REJECT_COMMAND or alias in REJECT_ALIASES:
        kind = "reject"
    elif body.startswith(NARROW_PREFIX):
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
            kind = "narrow"
            narrowed_scope = candidate
    return HumanDecision(
        kind=kind,
        actor=TRUSTED_HUMAN_ACTOR,
        comment_id=latest["id"],
        comment_url=latest["html_url"],
        body_sha256=hashlib.sha256(latest["body"].encode()).hexdigest(),
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
    observed_evidence = _observed_human_evidence(events)
    try:
        planner = parse_planner_evidence(comments)
        decision = parse_human_decision(comments)
        prior_narrowing = _latest_narrowing(events, comments)
    except NarrowingEvidenceError:
        if has_accepted_decision:
            return _history_regression_result(store, case)
        raise
    except PinnedEvidenceError:
        if has_accepted_decision:
            result = _history_regression_result(store, case)
            result["error"] = "pinned_planner_evidence_invalid"
            return result
        raise
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
            stale_prior = any(
                "HUMAN_APPROVED_STALE_BASE" in event["reason"] for event in prior
            )
            if current_base_sha != planner.planned_base_sha:
                if current_state == "BUILDING":
                    current_state = store.set_state(
                        CANARY_CASE_ID,
                        "BUILDING",
                        "RETURN_TO_PLANNING",
                        f"APPROVED_PLAN_BASE_BECAME_STALE {marker}",
                    ).value
                action = "replan"
                result["replan_reason"] = "stale_base"
                result["current_base_sha"] = current_base_sha
            elif stale_prior and current_state == "PLANNING":
                action = "replan"
                result["replan_reason"] = "stale_base"
                result["current_base_sha"] = current_base_sha
            else:
                action = (
                    "dispatch_builder" if current_state == "BUILDING" else "continue"
                )
        elif decision.kind == "narrow":
            action = "replan" if current_state == "PLANNING" else "continue"
        else:
            action = "stop"
        if action in {"dispatch_builder", "replan"}:
            return _active_route(result, action, current_state)
        return {**result, "action": action, "state": current_state}
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
                if current_base_sha != planner.planned_base_sha:
                    state = store.set_state(
                        CANARY_CASE_ID,
                        "PLANNING",
                        "HUMAN_APPROVED_STALE_BASE",
                        f"HUMAN_APPROVED_STALE_BASE {marker}",
                    )
                    action = "replan"
                    result["replan_reason"] = "stale_base"
                    result["current_base_sha"] = current_base_sha
                else:
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
