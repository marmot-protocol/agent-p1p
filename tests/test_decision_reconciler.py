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


def _versioned_planner(*, version: int, base: str) -> dict[str, object]:
    body = (
        f"Pip planning result for #1240 — plan v{version} "
        "(`NEEDS_HUMAN_SCOPE_DECISION`)\n\n"
        f"Freshly revalidated against current `master` at `{base}`.\n\n"
        "Proposed boundary: projection only; no MLS, keys, trust, or authorization changes."
    )
    return _comment(
        CANARY_PLANNER_COMMENT_ID,
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


def test_stale_planned_base_never_dispatches_builder(
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

    assert first["action"] == "replan"
    assert first["state"] == "PLANNING"
    assert first["replan_reason"] == "stale_base"
    assert first["current_base_sha"] == "b" * 40
    assert repeated["action"] == "replan"
    assert repeated["state"] == "PLANNING"
    assert store.get_case("mdk#1240")["plan_version"] == 1
    assert (
        sum(
            "HUMAN_APPROVED_STALE_BASE" in event["reason"]
            for event in store.list_events("mdk#1240")
        )
        == 1
    )


def test_accepted_approval_returns_to_planning_when_base_later_changes(
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
    assert replay["action"] == "replan"
    assert replay["state"] == "PLANNING"
    assert replay["replan_reason"] == "stale_base"
    assert replay["route_id"] != first["route_id"]


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
