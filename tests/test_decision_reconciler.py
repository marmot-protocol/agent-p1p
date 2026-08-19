from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Self

import pytest

from pip_agent.case_store import CaseStore
from pip_agent.decision_reconciler import (
    CANARY_PLANNER_COMMENT_ID,
    DecisionError,
    HumanDecision,
    fetch_canary_base_sha,
    fetch_canary_comments,
    fetch_canary_issue_authorization,
    parse_human_decision,
    parse_planner_evidence,
    reconcile_human_decision,
)

PLANNER_BODY = "canonical planner body"


class _Response:
    def __init__(self, payload: bytes) -> None:
        self.payload = payload

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_args: object) -> None:
        return None

    def read(self, limit: int) -> bytes:
        return self.payload[:limit]


@pytest.fixture(autouse=True)
def _github_credential(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    (tmp_path / "github.token").write_text("test_public_read_token\n")
    monkeypatch.setenv("CREDENTIALS_DIRECTORY", str(tmp_path))


def test_fetch_canary_comments_uses_only_fixed_bounded_endpoint() -> None:
    import json

    seen: list[tuple[str, int]] = []

    def opener(request: object, *, timeout: int) -> _Response:
        seen.append((request.full_url, timeout))  # type: ignore[attr-defined]
        return _Response(json.dumps([{"id": 1}]).encode())

    assert fetch_canary_comments(opener=opener) == [{"id": 1}]
    assert seen == [
        (
            (
                "https://api.github.com/repos/marmot-protocol/mdk/issues/1240/"
                "comments?per_page=100&page=1"
            ),
            10,
        )
    ]


def test_fetch_canary_comments_fails_closed_instead_of_paginating() -> None:
    calls = 0

    def opener(request: object, *, timeout: int) -> _Response:
        nonlocal calls
        calls += 1
        return _Response(json.dumps([{"id": value} for value in range(100)]).encode())

    with pytest.raises(DecisionError, match="pagination limit exceeded"):
        fetch_canary_comments(opener=opener)
    assert calls == 1


def test_fetch_canary_comments_uses_systemd_github_credential(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "github.token").write_text("test_public_read_token\n")
    monkeypatch.setenv("CREDENTIALS_DIRECTORY", str(tmp_path))

    def opener(request: object, *, timeout: int) -> _Response:
        assert request.get_header("Authorization") == "Bearer test_public_read_token"  # type: ignore[attr-defined]
        return _Response(b"[]")

    assert fetch_canary_comments(opener=opener) == []


def test_fetch_canary_comments_requires_systemd_github_credential(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("CREDENTIALS_DIRECTORY")
    with pytest.raises(DecisionError, match="GitHub credential is unavailable"):
        fetch_canary_comments(opener=lambda *_args, **_kwargs: None)


def test_fetch_canary_comments_rejects_multiline_credential(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (tmp_path / "github.token").write_text("first\nsecond\n")
    monkeypatch.setenv("CREDENTIALS_DIRECTORY", str(tmp_path))

    with pytest.raises(DecisionError, match="GitHub credential is malformed"):
        fetch_canary_comments(opener=lambda *_args, **_kwargs: None)


def test_fetch_canary_base_uses_only_fixed_bounded_endpoint() -> None:
    import json

    seen: list[tuple[str, int]] = []

    def opener(request: object, *, timeout: int) -> _Response:
        seen.append((request.full_url, timeout))  # type: ignore[attr-defined]
        return _Response(json.dumps({"sha": "a" * 40}).encode())

    assert fetch_canary_base_sha(opener=opener) == "a" * 40
    assert seen == [
        ("https://api.github.com/repos/marmot-protocol/mdk/commits/master", 10)
    ]


def test_fetch_canary_issue_authorization_requires_open_pip_ok() -> None:
    def opener(request: object, *, timeout: int) -> _Response:
        assert request.full_url.endswith("/issues/1240")  # type: ignore[attr-defined]
        assert timeout == 10
        return _Response(
            json.dumps(
                {"number": 1240, "state": "open", "labels": [{"name": "pip-ok"}]}
            ).encode()
        )

    assert fetch_canary_issue_authorization(opener=opener) is None

    def revoked(_request: object, *, timeout: int) -> _Response:
        assert timeout == 10
        return _Response(
            json.dumps({"number": 1240, "state": "open", "labels": []}).encode()
        )

    with pytest.raises(DecisionError, match="pip-ok"):
        fetch_canary_issue_authorization(opener=revoked)


def test_fetch_canary_comments_rejects_oversized_or_non_list_response() -> None:
    from pip_agent.decision_reconciler import MAX_GITHUB_RESPONSE_BYTES

    with pytest.raises(DecisionError, match="too large"):
        fetch_canary_comments(
            opener=lambda *_args, **_kwargs: _Response(
                b"x" * (MAX_GITHUB_RESPONSE_BYTES + 1)
            )
        )
    with pytest.raises(DecisionError, match="not a comment list"):
        fetch_canary_comments(opener=lambda *_args, **_kwargs: _Response(b"{}"))


def _comment(
    comment_id: int,
    author: str,
    body: str,
    *,
    created_at: str,
) -> dict[str, object]:
    actor_id = {
        "agent-p1p": 292420120,
        "erskingardner": 202880,
    }.get(author, 999999)
    return {
        "id": comment_id,
        "user": {"login": author, "id": actor_id},
        "body": body,
        "created_at": created_at,
        "updated_at": created_at,
        "html_url": (
            "https://github.com/marmot-protocol/mdk/issues/1240"
            f"#issuecomment-{comment_id}"
        ),
    }


def _planner() -> dict[str, object]:
    return _comment(
        CANARY_PLANNER_COMMENT_ID,
        "agent-p1p",
        PLANNER_BODY,
        created_at="2026-08-15T21:06:24Z",
    )


def _versioned_planner(
    *, version: int, base: str, outcome: str = "NEEDS_HUMAN_SCOPE_DECISION"
) -> dict[str, object]:
    body = (
        f"Pip planning result for #1240 — plan v{version} "
        f"(`{outcome}`)\n\n"
        f"Freshly revalidated against current `master` at `{base}`.\n\n"
        "Proposed boundary: projection only; no MLS, keys, trust, or authorization changes."
    )
    if outcome == "PROCEED":
        binding = json.dumps(
            {
                "authorized_scope": "repair repository-local projection delivery",
                "dependencies": [],
                "open_decisions": [],
                "outcome": outcome,
                "plan_version": version,
                "sensitive_scope": [],
                "task_id": f"t_plan{version}",
            },
            separators=(",", ":"),
            sort_keys=True,
        )
        body += f"\n\nPip execution binding: {binding}"
    return _comment(
        CANARY_PLANNER_COMMENT_ID + version * 1_000,
        "agent-p1p",
        body,
        created_at="2026-08-15T21:06:24Z",
    )


def test_parse_human_decision_requires_pinned_planner_comment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        "0" * 64,
    )
    with pytest.raises(DecisionError, match="planner comment digest"):
        parse_human_decision([_planner()])


def test_parse_human_decision_accepts_only_explicit_trusted_commands(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    planner = _planner()
    wrong_actor = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "someone-else",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    ambiguous = _comment(
        CANARY_PLANNER_COMMENT_ID + 2,
        "erskingardner",
        "looks fine to me",
        created_at="2026-08-15T21:08:00Z",
    )
    assert parse_human_decision([planner, wrong_actor, ambiguous]) == HumanDecision(
        kind="unrecognized",
        actor="erskingardner",
        comment_id=CANARY_PLANNER_COMMENT_ID + 2,
        comment_url=(
            "https://github.com/marmot-protocol/mdk/issues/1240"
            f"#issuecomment-{CANARY_PLANNER_COMMENT_ID + 2}"
        ),
        body_sha256=hashlib.sha256(b"looks fine to me").hexdigest(),
        narrowed_scope=None,
    )


@pytest.mark.parametrize(
    ("body", "expected_kind"),
    [
        ("approve", "approve"),
        ("approve\n", "approve"),
        ("approved", "approve"),
        ("Approve", "approve"),
        ("@agent-p1p approve", "approve"),
        ("@agent-p1p approved", "approve"),
        ("reject", "reject"),
        ("reject\n", "reject"),
        ("rejected", "reject"),
        ("@agent-p1p reject", "reject"),
        ("@agent-p1p rejected", "reject"),
    ],
)
def test_parse_human_decision_accepts_simple_whole_comment_aliases(
    body: str, expected_kind: str
) -> None:
    decision = parse_human_decision(
        [
            _versioned_planner(version=3, base="a" * 40),
            _comment(
                CANARY_PLANNER_COMMENT_ID + 1,
                "erskingardner",
                body,
                created_at="2026-08-15T21:07:00Z",
            ),
        ]
    )
    assert decision is not None
    assert decision.kind == expected_kind


@pytest.mark.parametrize(
    "body",
    [
        "approved, thanks",
        "not approved",
        "approve\nplease",
        "\napprove",
        "approve\n\n",
        "approve\r",
        "\rapprove",
        "approve\r\n",
        "\nreject",
        "reject\n\n",
        "reject\r",
        "\rreject",
        "reject\r\n",
    ],
)
def test_parse_human_decision_rejects_ambiguous_aliases(body: str) -> None:
    decision = parse_human_decision(
        [
            _versioned_planner(version=3, base="a" * 40),
            _comment(
                CANARY_PLANNER_COMMENT_ID + 1,
                "erskingardner",
                body,
                created_at="2026-08-15T21:07:00Z",
            ),
        ]
    )
    assert decision is not None
    assert decision.kind == "unrecognized"


def test_parse_human_decision_routes_latest_trusted_command(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    planner = _planner()
    approve = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reject = _comment(
        CANARY_PLANNER_COMMENT_ID + 2,
        "erskingardner",
        "Pip: reject",
        created_at="2026-08-15T21:08:00Z",
    )
    decision = parse_human_decision([planner, approve, reject])
    assert decision is not None
    assert decision.kind == "reject"
    assert decision.comment_id == CANARY_PLANNER_COMMENT_ID + 2


def test_parse_human_decision_preserves_explicit_narrowed_scope(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    decision = parse_human_decision(
        [
            _planner(),
            _comment(
                CANARY_PLANNER_COMMENT_ID + 1,
                "erskingardner",
                "Pip: narrow scope — projection only; no wake changes",
                created_at="2026-08-15T21:07:00Z",
            ),
        ]
    )
    assert decision is not None
    assert decision.kind == "narrow"
    assert decision.narrowed_scope == "projection only; no wake changes"


def test_trusted_login_with_wrong_numeric_identity_is_not_authoritative(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    forged = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    forged["user"] = {"login": "erskingardner", "id": 999999}
    assert parse_human_decision([_planner(), forged]) is None


def test_narrowing_with_credential_like_content_is_not_routed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    decision = parse_human_decision(
        [
            _planner(),
            _comment(
                CANARY_PLANNER_COMMENT_ID + 1,
                "erskingardner",
                "Pip: narrow scope — ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456",
                created_at="2026-08-15T21:07:00Z",
            ),
        ]
    )
    assert decision is not None
    assert decision.kind == "unrecognized"
    assert decision.narrowed_scope is None


def test_reconcile_approval_is_durable_idempotent_and_routes_builder(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    comments = [
        _planner(),
        _comment(
            CANARY_PLANNER_COMMENT_ID + 1,
            "erskingardner",
            "Pip: approve exact scope",
            created_at="2026-08-15T21:07:00Z",
        ),
    ]

    first = reconcile_human_decision(store, comments)
    second = reconcile_human_decision(store, comments)

    assert first == second
    assert first["action"] == "dispatch_builder"
    assert first["state"] == "BUILDING"
    assert first["route_id"].startswith("decision-")
    assert len(first["route_id"]) == len("decision-") + 64
    assert store.get_case("mdk#1240")["plan_version"] == 1
    events = store.list_events("mdk#1240")
    assert sum("HUMAN_APPROVED_SCOPE" in event["reason"] for event in events) == 1
    assert str(CANARY_PLANNER_COMMENT_ID + 1) in events[-1]["reason"]


def test_planned_base_drift_does_not_block_human_approved_builder(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    comments = [
        _planner(),
        _comment(
            CANARY_PLANNER_COMMENT_ID + 1,
            "erskingardner",
            "Pip: approve exact scope",
            created_at="2026-08-15T21:07:00Z",
        ),
    ]

    first = reconcile_human_decision(store, comments, current_base_sha="b" * 40)
    repeated = reconcile_human_decision(store, comments, current_base_sha="c" * 40)

    assert first["action"] == "dispatch_builder"
    assert first["state"] == "BUILDING"
    assert repeated == first
    assert store.get_case("mdk#1240")["plan_version"] == 1


def test_accepted_approval_remains_stable_when_base_later_changes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    comments = [
        _planner(),
        _comment(
            CANARY_PLANNER_COMMENT_ID + 1,
            "erskingardner",
            "Pip: approve exact scope",
            created_at="2026-08-15T21:07:00Z",
        ),
    ]

    first = reconcile_human_decision(store, comments)
    replay = reconcile_human_decision(store, comments, current_base_sha="f" * 40)

    assert first["action"] == "dispatch_builder"
    assert replay == first


def test_ordinary_proceed_plan_auto_dispatches_without_human_decision(
    tmp_path: Path,
) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    planner = _versioned_planner(version=1, base="a" * 40, outcome="PROCEED")

    first = reconcile_human_decision(store, [planner], current_base_sha="b" * 40)
    replay = reconcile_human_decision(store, [planner], current_base_sha="c" * 40)

    assert first["action"] == "dispatch_builder"
    assert first["decision"] == "automatic"
    assert first["state"] == "BUILDING"
    assert replay == first
    assert store.get_case("mdk#1240")["plan_version"] == 1
    assert store.get_case("mdk#1240")["state"] == "BUILDING"


def test_automatic_proceed_requires_machine_execution_binding(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    planner["body"] = str(planner["body"]).split("\n\nPip execution binding:", 1)[0]

    with pytest.raises(DecisionError, match="requires one execution binding"):
        reconcile_human_decision(store, [planner])


@pytest.mark.parametrize(
    "scope",
    [
        "change MLS keys and membership authorization semantics",
        "rotate key material for message encryption",
        "replace the trust anchor",
        "change admin authorization semantics",
        "change push-payload context construction",
        "change authentication and session authorization flows",
        "modify credential storage for notification delivery",
        "change authorization semantics for membership notification events",
        "change encrypted push notification metadata construction",
    ],
)
def test_sensitive_scope_cannot_auto_dispatch(tmp_path: Path, scope: str) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    prefix, raw_binding = str(planner["body"]).rsplit("Pip execution binding: ", 1)
    binding = json.loads(raw_binding)
    binding["authorized_scope"] = scope
    planner["body"] = (
        prefix
        + "Pip execution binding: "
        + json.dumps(binding, separators=(",", ":"), sort_keys=True)
    )

    with pytest.raises(DecisionError, match="execution binding is invalid"):
        reconcile_human_decision(store, [planner])


def test_edited_older_immutable_plan_cannot_override_newer_version() -> None:
    older = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    newer = _versioned_planner(version=3, base="b" * 40, outcome="PROCEED")
    newer["created_at"] = newer["updated_at"] = "2026-08-15T21:08:00Z"
    older["updated_at"] = "2026-08-15T21:09:00Z"

    with pytest.raises(DecisionError, match="immutable planner comment was edited"):
        parse_planner_evidence([newer, older])


def test_pinned_legacy_comment_id_does_not_exempt_changed_body() -> None:
    changed = _versioned_planner(version=1, base="a" * 40, outcome="PROCEED")
    changed["id"] = CANARY_PLANNER_COMMENT_ID
    changed["html_url"] = (
        "https://github.com/marmot-protocol/mdk/issues/1240"
        f"#issuecomment-{CANARY_PLANNER_COMMENT_ID}"
    )

    with pytest.raises(DecisionError, match="legacy planner comment digest"):
        parse_planner_evidence([changed])


def test_unedited_newer_plan_version_wins_over_timestamp_order() -> None:
    older = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    newer = _versioned_planner(version=3, base="b" * 40, outcome="PROCEED")
    older["created_at"] = older["updated_at"] = "2026-08-15T21:09:00Z"
    newer["created_at"] = newer["updated_at"] = "2026-08-15T21:08:00Z"

    evidence = parse_planner_evidence([newer, older])

    assert evidence.plan_version == 3
    assert evidence.planned_base_sha == "b" * 40


def test_exact_legacy_mutable_plan_can_migrate_to_immutable_new_version(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    legacy = _versioned_planner(version=5, base="a" * 40, outcome="PROCEED")
    legacy["id"] = 5331190946
    legacy["html_url"] = (
        "https://github.com/marmot-protocol/mdk/issues/1240#issuecomment-5331190946"
    )
    legacy["updated_at"] = "2026-08-15T21:09:00Z"
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.LEGACY_MUTABLE_PLANNER_COMMENTS",
        {legacy["id"]: hashlib.sha256(str(legacy["body"]).encode()).hexdigest()},
    )
    current = _versioned_planner(version=6, base="b" * 40, outcome="PROCEED")
    current["created_at"] = current["updated_at"] = "2026-08-15T21:10:00Z"

    evidence = parse_planner_evidence([legacy, current])

    assert evidence.plan_version == 6
    assert evidence.comment_id == current["id"]


def test_mutated_legacy_plan_fails_closed_even_with_new_version(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    legacy = _versioned_planner(version=5, base="a" * 40, outcome="PROCEED")
    legacy["id"] = 5331190946
    legacy["html_url"] = (
        "https://github.com/marmot-protocol/mdk/issues/1240#issuecomment-5331190946"
    )
    monkeypatch.setattr(
        "pip_agent.decision_reconciler.LEGACY_MUTABLE_PLANNER_COMMENTS",
        {legacy["id"]: "0" * 64},
    )
    current = _versioned_planner(version=6, base="b" * 40, outcome="PROCEED")

    with pytest.raises(DecisionError, match="legacy planner comment digest"):
        parse_planner_evidence([legacy, current])


def test_automatic_proceed_dispatches_builder_without_human_comment(
    tmp_path: Path,
) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    first_plan = _versioned_planner(version=1, base="a" * 40, outcome="PROCEED")
    first = reconcile_human_decision(store, [first_plan], current_base_sha="a" * 40)

    newer_plan = _versioned_planner(version=2, base="b" * 40, outcome="PROCEED")
    result = reconcile_human_decision(
        store, [first_plan, newer_plan], current_base_sha="b" * 40
    )

    assert result == first
    assert result["action"] == "dispatch_builder"
    assert result["state"] == "BUILDING"


def test_rejection_cannot_be_bypassed_by_a_newer_proceed_plan(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    rejection = _comment(
        CANARY_PLANNER_COMMENT_ID + 41,
        "erskingardner",
        "Pip: reject",
        created_at="2026-08-15T21:05:00Z",
    )
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")

    result = reconcile_human_decision(store, [rejection, planner])

    assert result["action"] == "stop"
    assert result["decision"] == "reject"
    assert result["state"] == "ABANDONED"


def test_unbound_narrowing_cannot_be_bypassed_by_a_newer_proceed_plan(
    tmp_path: Path,
) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    narrow = _comment(
        CANARY_PLANNER_COMMENT_ID + 42,
        "erskingardner",
        "Pip: narrow scope — projection only",
        created_at="2026-08-15T21:05:00Z",
    )
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")

    result = reconcile_human_decision(store, [narrow, planner])

    assert result["action"] == "replan"
    assert result["decision"] == "narrow"


def test_bound_narrowing_allows_newer_proceed_plan_to_auto_dispatch(
    tmp_path: Path,
) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    narrow = _comment(
        CANARY_PLANNER_COMMENT_ID + 43,
        "erskingardner",
        "Pip: narrow scope — projection only",
        created_at="2026-08-15T21:05:00Z",
    )
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    prefix, raw_execution = str(planner["body"]).rsplit("Pip execution binding: ", 1)
    execution = json.loads(raw_execution)
    execution["authorized_scope"] = "projection only"
    planner["body"] = (
        prefix
        + "Pip execution binding: "
        + json.dumps(execution, separators=(",", ":"), sort_keys=True)
    )
    binding = json.dumps(
        {
            "body_sha256": hashlib.sha256(str(narrow["body"]).encode()).hexdigest(),
            "comment_id": narrow["id"],
            "narrowed_scope": "projection only",
        },
        separators=(",", ":"),
        sort_keys=True,
    )
    planner["body"] = f"{planner['body']}\n\nPip narrowing binding: {binding}"

    result = reconcile_human_decision(store, [narrow, planner])

    assert result["action"] == "dispatch_builder"
    assert result["decision"] == "automatic"
    assert result["narrowed_scope"] == "projection only"
    assert result["authorized_scope"] == "projection only"


def test_narrowing_binding_must_exactly_bound_execution_scope(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    narrow = _comment(
        CANARY_PLANNER_COMMENT_ID + 44,
        "erskingardner",
        "Pip: narrow scope — tests only",
        created_at="2026-08-15T21:05:00Z",
    )
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    binding = json.dumps(
        {
            "body_sha256": hashlib.sha256(str(narrow["body"]).encode()).hexdigest(),
            "comment_id": narrow["id"],
            "narrowed_scope": "tests only",
        },
        separators=(",", ":"),
        sort_keys=True,
    )
    planner["body"] = f"{planner['body']}\n\nPip narrowing binding: {binding}"

    with pytest.raises(DecisionError, match="execution binding is invalid"):
        reconcile_human_decision(store, [narrow, planner])


def test_human_approval_does_not_retarget_automatic_proceed_plan(
    tmp_path: Path,
) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    planner = _versioned_planner(version=1, base="a" * 40, outcome="PROCEED")
    first = reconcile_human_decision(store, [planner], current_base_sha="a" * 40)
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )

    replay = reconcile_human_decision(
        store, [planner, approval], current_base_sha="b" * 40
    )

    assert replay == first
    assert replay["decision"] == "automatic"


@pytest.mark.parametrize(
    "outcome",
    [
        "ALREADY_FIXED",
        "NOT_REPRODUCIBLE",
        "DUPLICATE",
        "CROSS_REPO_DEPENDENCY",
        "ABANDON",
        "BLOCKED_UNEXPECTED_MODEL",
    ],
)
def test_approval_never_dispatches_terminal_or_dependency_outcome(
    tmp_path: Path, outcome: str
) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    planner = _versioned_planner(version=2, base="a" * 40, outcome=outcome)
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 50,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )

    result = reconcile_human_decision(store, [planner, approval])

    assert result["action"] == "wait"
    assert result["state"] == "PLANNING"
    assert result["reason"] == "planner_outcome_not_executable"


@pytest.mark.parametrize(
    ("created_at", "updated_at"),
    [
        ("not-a-timestamp", "not-a-timestamp"),
        ("2026-08-15T21:07:00+00:00", "2026-08-15T21:07:00+00:00"),
        ("2026-08-15T21:08:00Z", "2026-08-15T21:07:00Z"),
    ],
)
def test_malformed_or_regressing_comment_timestamps_fail_closed(
    created_at: str, updated_at: str
) -> None:
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 51,
        "erskingardner",
        "Pip: approve exact scope",
        created_at=created_at,
    )
    approval["updated_at"] = updated_at

    with pytest.raises(DecisionError, match="timestamp"):
        parse_human_decision([planner, approval])


def test_fractional_timestamp_order_is_chronological() -> None:
    planner = _versioned_planner(version=2, base="a" * 40, outcome="PROCEED")
    planner["created_at"] = "2026-08-15T21:07:00Z"
    planner["updated_at"] = "2026-08-15T21:07:00Z"
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 52,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00.100000Z",
    )

    decision = parse_human_decision([planner, approval])

    assert decision is not None
    assert decision.kind == "approve"


def test_human_rejection_supersedes_automatic_proceed_plan(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    planner = _versioned_planner(version=1, base="a" * 40, outcome="PROCEED")
    reconcile_human_decision(store, [planner], current_base_sha="a" * 40)
    rejection = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: reject",
        created_at="2026-08-15T21:07:00Z",
    )

    result = reconcile_human_decision(
        store, [planner, rejection], current_base_sha="b" * 40
    )

    assert result["action"] == "stop"
    assert result["state"] == "ABANDONED"


def test_reconcile_reject_abandons_without_builder_route(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    result = reconcile_human_decision(
        store,
        [
            _planner(),
            _comment(
                CANARY_PLANNER_COMMENT_ID + 1,
                "erskingardner",
                "Pip: reject",
                created_at="2026-08-15T21:07:00Z",
            ),
        ],
    )
    assert result["action"] == "stop"
    assert result["state"] == "ABANDONED"


def test_reconcile_narrowing_propagates_exact_scope_in_route(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")

    result = reconcile_human_decision(
        store,
        [
            _planner(),
            _comment(
                CANARY_PLANNER_COMMENT_ID + 1,
                "erskingardner",
                "Pip: narrow scope — projection only; no wake changes",
                created_at="2026-08-15T21:07:00Z",
            ),
        ],
    )

    assert result["action"] == "replan"
    assert result["narrowed_scope"] == "projection only; no wake changes"
    event = store.list_events("mdk#1240")[-1]["reason"]
    assert result["comment_url"] in event
    assert result["evidence_body_sha256"] in event


def test_later_human_rejection_stops_an_approved_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])
    rejection = _comment(
        CANARY_PLANNER_COMMENT_ID + 2,
        "erskingardner",
        "Pip: reject",
        created_at="2026-08-15T21:08:00Z",
    )

    result = reconcile_human_decision(store, [_planner(), approval, rejection])

    assert result["action"] == "stop"
    assert result["state"] == "ABANDONED"


def test_missing_or_edited_accepted_decision_blocks_active_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])

    result = reconcile_human_decision(store, [_planner()])

    assert result["action"] == "stop"
    assert result["state"] == "BLOCKED"
    assert "HISTORY_REGRESSED" in store.list_events("mdk#1240")[-1]["reason"]


def test_edited_pinned_planner_comment_blocks_active_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])
    edited_planner = _planner()
    edited_planner["body"] = "edited after approval"

    result = reconcile_human_decision(store, [edited_planner, approval])

    assert result["action"] == "stop"
    assert result["state"] == "BLOCKED"
    assert result["error"] == "pinned_planner_evidence_invalid"


def test_deleted_newer_narrowing_never_falls_back_to_older_approval(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    narrowing = _comment(
        CANARY_PLANNER_COMMENT_ID + 2,
        "erskingardner",
        "Pip: narrow scope — projection only",
        created_at="2026-08-15T21:08:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])
    narrowed = reconcile_human_decision(store, [_planner(), approval, narrowing])
    assert narrowed["state"] == "PLANNING"

    result = reconcile_human_decision(store, [_planner(), approval])

    assert result["action"] == "stop"
    assert result["state"] == "PLANNING"
    assert result["error"] == "human_decision_history_regressed"


def test_old_approval_does_not_redispatch_after_workflow_progresses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])
    store.set_state("mdk#1240", "BUILDING", "REVIEW_READY", "fixture")

    result = reconcile_human_decision(store, [_planner(), approval])

    assert result["action"] == "continue"
    assert result["state"] == "REVIEWING"


def test_narrowing_requires_a_new_planner_version_before_approval(
    tmp_path: Path,
) -> None:
    base = "d" * 40
    planner = _versioned_planner(version=2, base=base)
    narrow = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: narrow scope — projection only",
        created_at="2026-08-15T21:07:00Z",
    )
    approve = _comment(
        CANARY_PLANNER_COMMENT_ID + 2,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:08:00Z",
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")

    narrowed = reconcile_human_decision(store, [planner, narrow], current_base_sha=base)
    premature = reconcile_human_decision(
        store, [planner, narrow, approve], current_base_sha=base
    )

    assert narrowed["action"] == "replan"
    assert premature["action"] == "continue"
    assert premature["state"] == "PLANNING"
    assert premature["reason"] == "narrowed_plan_not_replaced"


def test_new_planner_version_resolves_narrowing_and_can_dispatch(
    tmp_path: Path,
) -> None:
    base = "e" * 40
    planner_v2 = _versioned_planner(version=2, base=base)
    narrow_body = "Pip: narrow scope — projection only"
    narrow = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        narrow_body,
        created_at="2026-08-15T21:07:00Z",
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    reconcile_human_decision(store, [planner_v2, narrow], current_base_sha=base)

    planner_v3 = _versioned_planner(version=3, base=base)
    planner_v3["created_at"] = "2026-08-15T21:08:00Z"
    planner_v3["updated_at"] = "2026-08-15T21:08:00Z"
    approve = _comment(
        CANARY_PLANNER_COMMENT_ID + 2,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:09:00Z",
    )
    unbound = reconcile_human_decision(
        store, [planner_v3, narrow, approve], current_base_sha=base
    )
    assert unbound["action"] == "continue"
    assert unbound["reason"] == "narrowed_plan_not_replaced"

    planner_v4 = _versioned_planner(version=4, base=base)
    planner_v4["body"] = (
        str(planner_v4["body"])
        + "\nPip narrowing binding: "
        + json.dumps(
            {
                "comment_id": narrow["id"],
                "body_sha256": hashlib.sha256(narrow_body.encode()).hexdigest(),
                "narrowed_scope": "projection only",
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )
    planner_v4["created_at"] = "2026-08-15T21:10:00Z"
    planner_v4["updated_at"] = "2026-08-15T21:10:00Z"
    approve_v4 = _comment(
        CANARY_PLANNER_COMMENT_ID + 3,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:11:00Z",
    )
    result = reconcile_human_decision(
        store, [planner_v4, narrow, approve_v4], current_base_sha=base
    )

    assert result["action"] == "dispatch_builder"
    assert result["plan_version"] == 4
    assert result["narrowed_scope"] == "projection only"
    assert result["planner_body_sha256"] != ""
    assert store.get_case("mdk#1240")["plan_version"] == 4


def test_editing_accepted_approval_blocks_review_instead_of_requesting_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])
    store.set_state("mdk#1240", "BUILDING", "REVIEW_READY", "fixture")
    edited = {**approval, "body": "not an approval anymore"}

    result = reconcile_human_decision(store, [_planner(), edited])

    assert result["action"] == "stop"
    assert result["state"] == "BLOCKED"
    assert result["error"] == "human_decision_evidence_mutated"


def test_editing_accepted_approval_to_recognized_reject_still_blocks_review(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hashlib

    monkeypatch.setattr(
        "pip_agent.decision_reconciler.CANARY_PLANNER_COMMENT_SHA256",
        hashlib.sha256(PLANNER_BODY.encode()).hexdigest(),
    )
    store = CaseStore(tmp_path / "cases.db")
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    approval = _comment(
        CANARY_PLANNER_COMMENT_ID + 1,
        "erskingardner",
        "Pip: approve exact scope",
        created_at="2026-08-15T21:07:00Z",
    )
    reconcile_human_decision(store, [_planner(), approval])
    store.set_state("mdk#1240", "BUILDING", "REVIEW_READY", "fixture")
    edited = {**approval, "body": "Pip: reject"}

    result = reconcile_human_decision(store, [_planner(), edited])

    assert result["action"] == "stop"
    assert result["state"] == "BLOCKED"
    assert result["error"] == "human_decision_evidence_mutated"
