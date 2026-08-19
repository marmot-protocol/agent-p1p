from __future__ import annotations

import argparse
import json
import os
import re
import socket
import socketserver
import stat
import struct
import threading
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .case_store import CaseStore, CaseStoreError
from .decision_reconciler import (
    DecisionError,
    fetch_canary_comments,
    reconcile_human_decision,
)

MAX_REQUEST_BYTES = 16_384
MAX_CONCURRENT_CLIENTS = 16
SERVICE_UNIT_TEMPLATE = """[Unit]
Description=Pip v2 isolated deterministic case writer
After=local-fs.target

[Service]
Type=notify
NotifyAccess=main
User=pip-v2-control
Group=pip-v2-control
SupplementaryGroups=@CALLER_GROUP@
Environment=PYTHONPATH=/opt/pip-v2/current
Environment=PYTHONDONTWRITEBYTECODE=1
Environment=PYTHONSAFEPATH=1
ExecStart=/usr/local/bin/pip-v2-control serve --config /etc/pip-v2/control.json
Restart=on-failure
RestartSec=5s
TimeoutStartSec=15s
UMask=0077
StateDirectory=pip-v2
StateDirectoryMode=0700
RuntimeDirectory=pip-v2
RuntimeDirectoryMode=0755
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ProtectClock=true
ProtectControlGroups=true
ProtectKernelLogs=true
ProtectKernelModules=true
ProtectKernelTunables=true
RestrictAddressFamilies=AF_UNIX
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=true
CapabilityBoundingSet=
AmbientCapabilities=
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
"""
DECISION_SERVICE_UNIT_TEMPLATE = """[Unit]
Description=Pip v2 exact-canary GitHub human-decision reconciler
After=network-online.target pip-v2-control.service
Wants=network-online.target
Requires=pip-v2-control.service

[Service]
Type=oneshot
User=pip-v2-control
Group=pip-v2-control
SupplementaryGroups=@CALLER_GROUP@
LoadCredential=github.token:/etc/pip-v2/github.token
Environment=PYTHONPATH=/opt/pip-v2/current
Environment=PYTHONDONTWRITEBYTECODE=1
Environment=PYTHONSAFEPATH=1
ExecStart=/usr/local/bin/pip-v2-control reconcile-once --config /etc/pip-v2/control.json --route-output /run/pip-v2/decision-route.json
UMask=0027
StateDirectory=pip-v2
StateDirectoryMode=0700
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=true
ProtectClock=true
ProtectControlGroups=true
ProtectKernelLogs=true
ProtectKernelModules=true
ProtectKernelTunables=true
RestrictAddressFamilies=AF_INET AF_INET6
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=true
CapabilityBoundingSet=
AmbientCapabilities=
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
"""
DECISION_TIMER_UNIT = """[Unit]
Description=Poll the exact Pip v2 canary for authoritative GitHub decisions

[Timer]
OnActiveSec=2m
# One authenticated comments read per pass; activation validation separately
# reads issue authorization and comments only while advancing active work.
OnUnitActiveSec=5m
RandomizedDelaySec=15s
Persistent=true
Unit=pip-v2-decision.service

[Install]
WantedBy=timers.target
"""
POLICY_KEYS = frozenset(
    {
        "repository",
        "issue_number",
        "intake_label",
        "merge_mode",
        "autonomous_merge",
        "state_database",
        "socket_path",
        "socket_group",
        "allowed_uids",
    }
)


class ControlPlaneError(RuntimeError):
    """The isolated Pip v2 control plane rejected a request."""


def render_service_unit(caller_group: str) -> str:
    if re.fullmatch(r"[A-Za-z0-9_.-]+", caller_group) is None:
        raise ControlPlaneError("control-plane caller group is invalid")
    if SERVICE_UNIT_TEMPLATE.count("@CALLER_GROUP@") != 1:
        raise ControlPlaneError("control-plane service template is invalid")
    return SERVICE_UNIT_TEMPLATE.replace("@CALLER_GROUP@", caller_group)


def render_decision_service_unit(caller_group: str) -> str:
    if re.fullmatch(r"[A-Za-z0-9_.-]+", caller_group) is None:
        raise ControlPlaneError("decision-service caller group is invalid")
    if DECISION_SERVICE_UNIT_TEMPLATE.count("@CALLER_GROUP@") != 1:
        raise ControlPlaneError("decision-service template is invalid")
    return DECISION_SERVICE_UNIT_TEMPLATE.replace("@CALLER_GROUP@", caller_group)


def render_decision_timer_unit() -> str:
    return DECISION_TIMER_UNIT


@dataclass(frozen=True)
class ControlPolicy:
    repository: str
    issue_number: int
    intake_label: str
    merge_mode: str
    autonomous_merge: bool
    state_database: Path
    socket_path: Path
    socket_group: int
    allowed_uids: tuple[int, ...]

    def __post_init__(self) -> None:
        if self.repository != "marmot-protocol/mdk":
            raise ControlPlaneError("control policy repository is not authorized")
        if type(self.issue_number) is not int or self.issue_number != 1240:
            raise ControlPlaneError("control policy is restricted to canary issue 1240")
        if self.intake_label != "pip-ok":
            raise ControlPlaneError("control policy intake label must be pip-ok")
        if self.merge_mode != "shadow" or self.autonomous_merge is not False:
            raise ControlPlaneError(
                "control policy must remain shadow and human-merge-only"
            )
        if not self.state_database.is_absolute() or not self.socket_path.is_absolute():
            raise ControlPlaneError("control-plane paths must be absolute")
        if type(self.socket_group) is not int or self.socket_group < 0:
            raise ControlPlaneError("control socket group is invalid")
        if not self.allowed_uids or any(
            type(uid) is not int or uid < 0 for uid in self.allowed_uids
        ):
            raise ControlPlaneError("control-plane caller UIDs are invalid")

    @property
    def case_id(self) -> str:
        return f"{self.repository.rsplit('/', 1)[-1]}#{self.issue_number}"


def load_policy(path: Path, *, require_root_owner: bool = True) -> ControlPolicy:
    if path.is_symlink():
        raise ControlPlaneError("control policy path must not be a symlink")
    try:
        metadata = path.stat()
    except FileNotFoundError as exc:
        raise ControlPlaneError("control policy does not exist") from exc
    if require_root_owner and metadata.st_uid != 0:
        raise ControlPlaneError("control policy must be owned by root")
    if stat.S_IMODE(metadata.st_mode) & 0o022:
        raise ControlPlaneError("control policy must not be group/other writable")
    if metadata.st_size > MAX_REQUEST_BYTES:
        raise ControlPlaneError("control policy is too large")
    try:
        payload = json.loads(path.read_text())
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ControlPlaneError("control policy is unreadable or malformed") from exc
    if not isinstance(payload, dict) or set(payload) != POLICY_KEYS:
        raise ControlPlaneError("control policy has invalid keys")
    allowed_uids = payload["allowed_uids"]
    if not isinstance(allowed_uids, list):
        raise ControlPlaneError("control policy allowed_uids must be a list")
    state_database = payload["state_database"]
    socket_path = payload["socket_path"]
    if not isinstance(state_database, str) or not isinstance(socket_path, str):
        raise ControlPlaneError("control policy paths must be strings")
    return ControlPolicy(
        repository=payload["repository"],
        issue_number=payload["issue_number"],
        intake_label=payload["intake_label"],
        merge_mode=payload["merge_mode"],
        autonomous_merge=payload["autonomous_merge"],
        state_database=Path(state_database),
        socket_path=Path(socket_path),
        socket_group=payload["socket_group"],
        allowed_uids=tuple(allowed_uids),
    )


class _ControlHandler(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        server = self.server
        assert isinstance(server, ControlServer)
        peer = self.request.getsockopt(
            socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")
        )
        _, uid, _ = struct.unpack("3i", peer)
        if uid not in server.policy.allowed_uids:
            server._write_response(
                self.wfile, {"ok": False, "error": "unauthorized peer"}
            )
            return
        self.request.settimeout(5)
        try:
            raw = self.rfile.readline(MAX_REQUEST_BYTES + 1)
        except (TimeoutError, OSError):
            server._write_response(
                self.wfile, {"ok": False, "error": "request read timed out"}
            )
            return
        if not raw or len(raw) > MAX_REQUEST_BYTES or not raw.endswith(b"\n"):
            server._write_response(
                self.wfile, {"ok": False, "error": "invalid request envelope"}
            )
            return
        try:
            request = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError):
            server._write_response(
                self.wfile, {"ok": False, "error": "malformed request JSON"}
            )
            return
        response = server.dispatch(request)
        server._write_response(self.wfile, response)


class ControlServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    allow_reuse_address = False
    daemon_threads = True
    request_queue_size = MAX_CONCURRENT_CLIENTS

    def __init__(self, policy: ControlPolicy) -> None:
        self.policy = policy
        self._client_slots = threading.BoundedSemaphore(MAX_CONCURRENT_CLIENTS)
        policy.socket_path.parent.mkdir(parents=True, exist_ok=True, mode=0o750)
        if policy.socket_path.is_symlink():
            raise ControlPlaneError("control socket path must not be a symlink")
        if policy.socket_path.exists():
            mode = policy.socket_path.lstat().st_mode
            if (
                not stat.S_ISSOCK(mode)
                or policy.socket_path.lstat().st_uid != os.geteuid()
            ):
                raise ControlPlaneError(
                    "refusing to replace foreign control socket path"
                )
            policy.socket_path.unlink()
        self.store = CaseStore(policy.state_database)
        super().__init__(str(policy.socket_path), _ControlHandler)
        os.chown(policy.socket_path, -1, policy.socket_group)
        policy.socket_path.chmod(0o660)

    def process_request(self, request: Any, client_address: Any) -> None:
        if not self._client_slots.acquire(blocking=False):
            try:
                request.sendall(b'{"error":"control service busy","ok":false}\n')
            except OSError:
                pass
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self._client_slots.release()
            raise

    def process_request_thread(self, request: Any, client_address: Any) -> None:
        try:
            super().process_request_thread(request, client_address)
        finally:
            self._client_slots.release()

    @staticmethod
    def _write_response(stream: Any, response: dict[str, Any]) -> None:
        try:
            stream.write(
                json.dumps(response, sort_keys=True, separators=(",", ":")).encode()
                + b"\n"
            )
        except OSError:
            return

    def dispatch(self, request: Any) -> dict[str, Any]:
        if not isinstance(request, dict) or set(request) != {"operation"}:
            return {"ok": False, "error": "request must contain only operation"}
        operation = request.get("operation")
        try:
            if operation == "ensure_canary":
                created = self.store.ensure_case(
                    self.policy.case_id,
                    self.policy.repository,
                    self.policy.issue_number,
                    self.policy.intake_label,
                )
                return {"ok": True, "created": created, "case_id": self.policy.case_id}
            if operation == "status":
                try:
                    case = self.store.get_case(self.policy.case_id)
                except CaseStoreError as exc:
                    if "unknown case" not in str(exc):
                        raise
                    case = None
                return {"ok": True, "case": case}
        except (CaseStoreError, DecisionError) as exc:
            return {"ok": False, "error": str(exc)}
        return {"ok": False, "error": "unsupported operation"}

    def server_close(self) -> None:
        super().server_close()
        try:
            self.policy.socket_path.unlink()
        except FileNotFoundError:
            pass


def control_request(socket_path: Path, request: dict[str, Any]) -> dict[str, Any]:
    encoded = (
        json.dumps(request, sort_keys=True, separators=(",", ":")).encode() + b"\n"
    )
    if len(encoded) > MAX_REQUEST_BYTES:
        raise ControlPlaneError("control request is too large")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(10)
        client.connect(str(socket_path))
        client.sendall(encoded)
        response_raw = b""
        while not response_raw.endswith(b"\n"):
            chunk = client.recv(MAX_REQUEST_BYTES + 1 - len(response_raw))
            if not chunk:
                break
            response_raw += chunk
            if len(response_raw) > MAX_REQUEST_BYTES:
                raise ControlPlaneError("control response is too large")
    try:
        response = json.loads(response_raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ControlPlaneError("control service returned malformed JSON") from exc
    if not isinstance(response, dict):
        raise ControlPlaneError("control service returned an invalid response")
    return response


def notify_systemd_ready() -> None:
    address = os.environ.pop("NOTIFY_SOCKET", None)
    if address is None:
        return
    if address.startswith("@"):
        address = "\0" + address[1:]
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as notifier:
        notifier.connect(address)
        notifier.sendall(b"READY=1\nSTATUS=Pip v2 control socket ready")


def _write_route_output(path: Path, payload: dict[str, Any], *, group_id: int) -> None:
    if path.is_symlink():
        raise ControlPlaneError("decision route output must not be a symlink")
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
    if path.exists():
        metadata = path.stat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid():
            raise ControlPlaneError("decision route output is not service-owned")
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    descriptor = os.open(
        temporary,
        os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW,
        0o640,
    )
    try:
        encoded = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode()
        view = memoryview(encoded)
        while view:
            written = os.write(descriptor, view)
            if written < 1:
                raise ControlPlaneError("decision route output write made no progress")
            view = view[written:]
        os.fsync(descriptor)
        os.fchown(descriptor, -1, group_id)
        os.fchmod(descriptor, 0o640)
        os.close(descriptor)
        descriptor = -1
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except BaseException:
        if descriptor >= 0:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def reconcile_once(
    policy: ControlPolicy,
    route_output: Path,
    *,
    comment_fetcher: Callable[[], list[dict[str, Any]]] = fetch_canary_comments,
) -> dict[str, Any]:
    store = CaseStore(policy.state_database)
    # A transient external lookup failure must prevent new activation without
    # destroying already-authorized active work. Keep the last durable route:
    # route_consumer performs its own live validation immediately before every
    # activation, while an already-running worker can finish and report.
    result = reconcile_human_decision(store, comment_fetcher())
    payload = {"ok": True, "case_id": policy.case_id, **result}
    _write_route_output(route_output, payload, group_id=policy.socket_group)
    return payload


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Pip v2 isolated case writer")
    commands = parser.add_subparsers(dest="command", required=True)
    serve = commands.add_parser("serve", help="Run the isolated writer service")
    serve.add_argument("--config", type=Path, required=True)
    request = commands.add_parser("request", help="Call the isolated writer service")
    request.add_argument("--socket", type=Path, required=True)
    request.add_argument(
        "--operation",
        choices=("ensure_canary", "status"),
        required=True,
    )
    reconcile_command = commands.add_parser(
        "reconcile-once", help="Fetch and reconcile the exact canary human decision"
    )
    reconcile_command.add_argument("--config", type=Path, required=True)
    reconcile_command.add_argument("--route-output", type=Path, required=True)
    render = commands.add_parser(
        "render-unit", help="Render the packaged hardened systemd unit"
    )
    render.add_argument("--caller-group", required=True)
    render.add_argument("--output", type=Path, required=True)
    render_decision = commands.add_parser(
        "render-decision-units",
        help="Render the exact-canary decision service and timer",
    )
    render_decision.add_argument("--caller-group", required=True)
    render_decision.add_argument("--service-output", type=Path, required=True)
    render_decision.add_argument("--timer-output", type=Path, required=True)
    args = parser.parse_args(argv)

    if args.command == "render-unit":
        args.output.write_text(render_service_unit(args.caller_group))
        return 0

    if args.command == "render-decision-units":
        args.service_output.write_text(render_decision_service_unit(args.caller_group))
        args.timer_output.write_text(render_decision_timer_unit())
        return 0

    if args.command == "reconcile-once":
        try:
            payload = reconcile_once(
                load_policy(args.config),
                args.route_output,
            )
        except (ControlPlaneError, CaseStoreError, DecisionError, OSError) as exc:
            parser.error(str(exc))
        print(json.dumps(payload, indent=2, sort_keys=True))
        return 0

    if args.command == "serve":
        policy = load_policy(args.config)
        server = ControlServer(policy)
        try:
            notify_systemd_ready()
            server.serve_forever()
        except KeyboardInterrupt:
            pass
        finally:
            server.server_close()
        return 0

    try:
        response = control_request(args.socket, {"operation": args.operation})
    except (ControlPlaneError, OSError) as exc:
        parser.error(str(exc))
    print(json.dumps(response, indent=2, sort_keys=True))
    return 0 if response.get("ok") is True else 1


if __name__ == "__main__":
    raise SystemExit(main())
