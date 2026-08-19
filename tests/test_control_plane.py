from __future__ import annotations

import json
import os
import socket
import stat
import subprocess
import threading
from pathlib import Path

import pytest

import pip_agent.decision_reconciler as decision_module
import pip_agent.intake as intake_module
from pip_agent.case_store import CaseStore
from pip_agent.control_plane import (
    MAX_CONCURRENT_CLIENTS,
    ControlPlaneError,
    ControlPolicy,
    ControlServer,
    control_request,
    load_policy,
    notify_systemd_ready,
    reconcile_once,
)
from pip_agent.control_plane import main as control_main
from pip_agent.intake import IntakeCandidate, IntakeError, ensure_controlled_case


def test_control_service_creates_only_the_configured_canary(tmp_path: Path) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "state" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    server = ControlServer(policy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        first = control_request(policy.socket_path, {"operation": "ensure_canary"})
        second = control_request(policy.socket_path, {"operation": "ensure_canary"})
        status_response = control_request(policy.socket_path, {"operation": "status"})
        socket_mode = stat.S_IMODE(policy.socket_path.stat().st_mode)
        database_mode = stat.S_IMODE(policy.state_database.stat().st_mode)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)

    assert first == {"ok": True, "created": True, "case_id": "mdk#1240"}
    assert second == {"ok": True, "created": False, "case_id": "mdk#1240"}
    assert status_response["ok"] is True
    assert status_response["case"]["repository"] == "marmot-protocol/mdk"
    assert status_response["case"]["issue_number"] == 1240
    assert status_response["case"]["state"] == "PLANNING"
    assert socket_mode == 0o660
    assert database_mode == 0o600


def test_control_service_reconciles_only_live_fixed_github_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    planner_body = "pinned plan"
    monkeypatch.setattr(
        decision_module,
        "CANARY_PLANNER_COMMENT_SHA256",
        __import__("hashlib").sha256(planner_body.encode()).hexdigest(),
    )
    comments = [
        {
            "id": decision_module.CANARY_PLANNER_COMMENT_ID,
            "user": {"login": "agent-p1p", "id": 292420120},
            "body": planner_body,
            "created_at": "2026-08-15T21:06:24Z",
            "updated_at": "2026-08-15T21:06:24Z",
            "html_url": (
                "https://github.com/marmot-protocol/mdk/issues/1240"
                f"#issuecomment-{decision_module.CANARY_PLANNER_COMMENT_ID}"
            ),
        },
        {
            "id": decision_module.CANARY_PLANNER_COMMENT_ID + 1,
            "user": {"login": "erskingardner", "id": 202880},
            "body": "Pip: approve exact scope",
            "created_at": "2026-08-15T21:07:00Z",
            "updated_at": "2026-08-15T21:07:00Z",
            "html_url": (
                "https://github.com/marmot-protocol/mdk/issues/1240"
                f"#issuecomment-{decision_module.CANARY_PLANNER_COMMENT_ID + 1}"
            ),
        },
    ]
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "state" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    store = CaseStore(policy.state_database)
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    route_output = tmp_path / "run" / "decision-route.json"
    response = reconcile_once(
        policy,
        route_output,
        comment_fetcher=lambda: comments,
    )
    repeated = reconcile_once(
        policy,
        route_output,
        comment_fetcher=lambda: comments,
    )

    assert response["ok"] is True
    assert response["action"] == "dispatch_builder"
    assert response["state"] == "BUILDING"
    assert repeated == response
    assert json.loads(route_output.read_text()) == response
    assert stat.S_IMODE(route_output.stat().st_mode) == 0o640


def test_reconcile_failure_replaces_stale_dispatch_route_with_stop(
    tmp_path: Path,
) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "state" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    store = CaseStore(policy.state_database)
    store.create_case("mdk#1240", "marmot-protocol/mdk", 1240, "pip-ok")
    route_output = tmp_path / "run" / "decision-route.json"
    route_output.parent.mkdir(parents=True)
    prior_route = b'{"action":"return_to_planning","route_id":"keep-running"}\n'
    route_output.write_bytes(prior_route)

    def fail() -> list[dict]:
        raise decision_module.DecisionError("fixture lookup failed")

    with pytest.raises(decision_module.DecisionError, match="fixture"):
        reconcile_once(policy, route_output, comment_fetcher=fail)

    assert route_output.read_bytes() == prior_route


def test_control_policy_and_protocol_fail_closed(tmp_path: Path) -> None:
    with pytest.raises(ControlPlaneError, match="shadow"):
        ControlPolicy(
            repository="marmot-protocol/mdk",
            issue_number=1240,
            intake_label="pip-ok",
            merge_mode="merge",
            autonomous_merge=True,
            state_database=tmp_path / "cases.db",
            socket_path=tmp_path / "control.sock",
            socket_group=os.getgid(),
            allowed_uids=(os.getuid(),),
        )

    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "state" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    server = ControlServer(policy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        extra = control_request(
            policy.socket_path,
            {"operation": "ensure_canary", "issue_number": 9999},
        )
        unsupported = control_request(policy.socket_path, {"operation": "set_state"})
        status_response = control_request(policy.socket_path, {"operation": "status"})
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)

    assert extra == {"ok": False, "error": "request must contain only operation"}
    assert unsupported == {"ok": False, "error": "unsupported operation"}
    assert status_response == {"ok": True, "case": None}
    assert not policy.state_database.exists() or status_response["case"] is None


def test_control_policy_loads_strict_root_install_shape(tmp_path: Path) -> None:
    config = tmp_path / "control.json"
    payload = {
        "repository": "marmot-protocol/mdk",
        "issue_number": 1240,
        "intake_label": "pip-ok",
        "merge_mode": "shadow",
        "autonomous_merge": False,
        "state_database": str(tmp_path / "state" / "cases.db"),
        "socket_path": str(tmp_path / "run" / "control.sock"),
        "socket_group": os.getgid(),
        "allowed_uids": [os.getuid()],
    }
    config.write_text(json.dumps(payload))
    config.chmod(0o644)

    policy = load_policy(config, require_root_owner=False)
    assert policy.case_id == "mdk#1240"

    payload["unexpected"] = True
    config.write_text(json.dumps(payload))
    with pytest.raises(ControlPlaneError, match="keys"):
        load_policy(config, require_root_owner=False)


def test_control_policy_rejects_every_issue_except_the_authorized_canary(
    tmp_path: Path,
) -> None:
    with pytest.raises(ControlPlaneError, match="issue 1240"):
        ControlPolicy(
            repository="marmot-protocol/mdk",
            issue_number=1241,
            intake_label="pip-ok",
            merge_mode="shadow",
            autonomous_merge=False,
            state_database=tmp_path / "cases.db",
            socket_path=tmp_path / "control.sock",
            socket_group=os.getgid(),
            allowed_uids=(os.getuid(),),
        )


def test_intake_uses_control_service_without_receiving_database_path(
    tmp_path: Path,
) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "private" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    candidate = IntakeCandidate(
        case_id="mdk#1240",
        repository="marmot-protocol/mdk",
        issue_number=1240,
        title="Fixture",
        url="https://github.com/marmot-protocol/mdk/issues/1240",
        authorization_actor="agent-p1p",
        intake_label="pip-ok",
    )
    server = ControlServer(policy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        assert ensure_controlled_case(policy.socket_path, candidate) is True
        assert ensure_controlled_case(policy.socket_path, candidate) is False
        wrong = IntakeCandidate(
            **{**candidate.__dict__, "case_id": "mdk#9999", "issue_number": 9999}
        )
        with pytest.raises(IntakeError, match="different case"):
            ensure_controlled_case(policy.socket_path, wrong)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_control_cli_requests_status_over_unix_socket(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "private" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    server = ControlServer(policy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        result = control_main(
            ["request", "--socket", str(policy.socket_path), "--operation", "status"]
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)

    assert result == 0
    assert json.loads(capsys.readouterr().out) == {"ok": True, "case": None}


def test_stalled_same_uid_client_cannot_block_other_control_requests(
    tmp_path: Path,
) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "cases.db",
        socket_path=tmp_path / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    server = ControlServer(policy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    stalled = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        stalled.connect(str(policy.socket_path))
        response = control_request(policy.socket_path, {"operation": "status"})
        assert response == {"ok": True, "case": None}
    finally:
        stalled.close()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_control_server_rejects_clients_beyond_fixed_concurrency_bound(
    tmp_path: Path,
) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "cases.db",
        socket_path=tmp_path / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    server = ControlServer(policy)
    for _ in range(MAX_CONCURRENT_CLIENTS):
        assert server._client_slots.acquire(blocking=False)
    server_side, client_side = socket.socketpair()
    try:
        server.process_request(server_side, None)
        assert json.loads(client_side.recv(1024)) == {
            "ok": False,
            "error": "control service busy",
        }
    finally:
        client_side.close()
        for _ in range(MAX_CONCURRENT_CLIENTS):
            server._client_slots.release()
        server.server_close()


def test_systemd_ready_notification_is_sent_to_configured_socket(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    notify_path = tmp_path / "notify.sock"
    receiver = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    receiver.bind(str(notify_path))
    receiver.settimeout(1)
    monkeypatch.setenv("NOTIFY_SOCKET", str(notify_path))
    try:
        notify_systemd_ready()
        assert receiver.recv(1024).startswith(b"READY=1\n")
        assert "NOTIFY_SOCKET" not in os.environ
    finally:
        receiver.close()


def test_intake_enqueue_uses_control_socket(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    policy = ControlPolicy(
        repository="marmot-protocol/mdk",
        issue_number=1240,
        intake_label="pip-ok",
        merge_mode="shadow",
        autonomous_merge=False,
        state_database=tmp_path / "private" / "cases.db",
        socket_path=tmp_path / "run" / "control.sock",
        socket_group=os.getgid(),
        allowed_uids=(os.getuid(),),
    )
    config = json.loads(
        (Path(__file__).parents[1] / "config/repositories/mdk.json").read_text()
    )
    config.update(
        new_intake_enabled=True,
        dispatch_enabled=True,
        canary_issue_number=1240,
    )
    config_path = tmp_path / "canary.json"
    config_path.write_text(json.dumps(config))
    issue = {
        "number": 1240,
        "state": "open",
        "title": "Fixture",
        "html_url": "https://github.com/marmot-protocol/mdk/issues/1240",
        "labels": [{"name": "pip-ok"}],
    }
    timeline = [
        {
            "event": "labeled",
            "label": {"name": "pip-ok"},
            "actor": {"login": "agent-p1p"},
        }
    ]
    monkeypatch.setattr(
        intake_module,
        "_gh_json",
        lambda endpoint, **kwargs: timeline if "timeline" in endpoint else issue,
    )
    subprocess_calls: list[list[str]] = []

    def fake_run(
        command: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        subprocess_calls.append(command)
        if "create" in command:
            stdout = json.dumps({"id": "planner-task", "status": "blocked"})
        else:
            stdout = "ok\n"
        return subprocess.CompletedProcess(command, 0, stdout, "")

    monkeypatch.setattr(intake_module.subprocess, "run", fake_run)
    server = ControlServer(policy)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        result = intake_module.main(
            [
                "--config",
                str(config_path),
                "--issue",
                "1240",
                "--control-socket",
                str(policy.socket_path),
                "--enqueue",
            ]
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)

    output = json.loads(capsys.readouterr().out)
    assert result == 0
    assert output["task"] == {"id": "planner-task", "status": "blocked"}
    assert [command[4] for command in subprocess_calls] == [
        "create",
        "notify-subscribe",
        "unblock",
    ]
    subscribe = subprocess_calls[1]
    assert subscribe[5] == "planner-task"
    assert subscribe[subscribe.index("--platform") + 1] == "telegram"
    assert subscribe[subscribe.index("--chat-id") + 1] == "483923125"
    assert subscribe[subscribe.index("--notifier-profile") + 1] == "default"
    assert CaseStore(policy.state_database).get_case("mdk#1240")["state"] == "PLANNING"
