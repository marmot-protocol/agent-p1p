from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from .contracts import ContractError, validate_contract
from .manifests import RoleManifest


class AdapterError(RuntimeError):
    """A deterministic provider adapter failed or returned unsafe output."""


class ProcessTimeout(AdapterError):
    def __init__(self, message: str, stdout: str, stderr: str) -> None:
        super().__init__(message)
        self.stdout = stdout
        self.stderr = stderr


MAX_CAPTURE_BYTES = 10_485_760


class CursorAdapter:
    def __init__(
        self,
        repo_root: Path,
        manifest: RoleManifest,
        *,
        executable: Path | str = "agent",
    ) -> None:
        if manifest.execution != "cursor":
            raise AdapterError(f"{manifest.name} is not a direct Cursor role")
        self.repo_root = repo_root.resolve()
        self.manifest = manifest
        self.executable = str(executable)

    def render_prompt(self, task: Mapping[str, Any]) -> str:
        parts = [
            self._skill_text("workflow-contract"),
            self._skill_text(self.manifest.name),
            "# Immutable Task Input\n\n```json\n"
            + json.dumps(dict(task), indent=2, sort_keys=True)
            + "\n```\n",
            (
                "# Result Requirement\n\n"
                f"Return only one JSON object valid against `{self.manifest.result_schema}`. "
                f"Set `requested_model` to `{self.manifest.contract_model}`. Report the model "
                "identity visible in your runtime context as `actual_model`; do not copy the "
                "requested value merely to satisfy this instruction. A mismatch must use "
                "`BLOCKED_UNEXPECTED_MODEL`.\n"
            ),
        ]
        return "\n\n".join(parts)

    def _skill_text(self, name: str) -> str:
        if name == "workflow-contract":
            path = self.repo_root / "skills/shared/workflow-contract/SKILL.md"
        else:
            path = self.repo_root / "skills" / name / "SKILL.md"
        if not path.is_file():
            raise AdapterError(f"required skill is missing: {path}")
        return path.read_text()

    def command(self, prompt: str, worktree: Path) -> list[str]:
        command = [
            self.executable,
            "--print",
            "--output-format",
            "json",
            "--model",
            self.manifest.model,
            "--workspace",
            str(worktree.resolve()),
            "--trust",
            "--sandbox",
            "enabled",
        ]
        if self.manifest.name == "builder-grok":
            command.append("--force")
        else:
            command.extend(["--mode", "plan"])
        command.append(prompt)
        return command

    def _preflight(self, artifact_dir: Path) -> dict[str, Any]:
        try:
            status = _run_process(
                [self.executable, "status", "--format", "json"],
                timeout=30,
            )
            models = _run_process(
                [self.executable, "models"],
                timeout=30,
            )
        except (OSError, AdapterError) as exc:
            raise AdapterError(f"Cursor preflight failed: {exc}") from exc
        if status.returncode != 0:
            raise AdapterError("Cursor authentication preflight failed")
        try:
            status_payload = json.loads(status.stdout)
        except json.JSONDecodeError as exc:
            raise AdapterError("Cursor status returned malformed JSON") from exc
        if status_payload.get("isAuthenticated") is not True:
            raise AdapterError("Cursor is not authenticated")
        entries = [
            line.split(" - ", 1)[0].strip()
            for line in models.stdout.splitlines()
            if " - " in line
        ]
        exact_count = entries.count(self.manifest.model)
        verification: dict[str, Any] = {
            "requested_model": self.manifest.model,
            "exact_model_advertised": exact_count == 1,
            "advertised_match_count": exact_count,
            "cli_reports_actual_model": False,
            "verification_level": "PINNED_REQUEST_NO_ROUTE_ATTESTATION",
            "limitation": (
                "Cursor CLI result envelopes do not expose the actual routed model; "
                "the adapter verifies exact availability, pins --model, and checks "
                "the result's self-reported model."
            ),
        }
        _write_artifact(
            artifact_dir / "model-verification.json",
            json.dumps(verification, indent=2, sort_keys=True) + "\n",
        )
        if models.returncode != 0 or exact_count != 1:
            raise AdapterError(
                f"pinned Cursor model {self.manifest.model!r} is not advertised exactly once"
            )
        return verification

    def run(
        self,
        task: Mapping[str, Any],
        worktree: Path,
        artifact_dir: Path,
        *,
        timeout: int = 3600,
    ) -> dict[str, Any]:
        if not worktree.is_dir():
            raise AdapterError(f"assigned worktree does not exist: {worktree}")
        _validate_task_input(self.manifest.name, self.manifest.result_schema, task)
        task_text = json.dumps(dict(task), indent=2, sort_keys=True) + "\n"
        if _contains_secret(task_text):
            raise AdapterError("immutable task input contains credential-like material")
        artifact_dir.mkdir(parents=True, exist_ok=False)
        artifact_dir.chmod(0o700)
        _write_artifact(
            artifact_dir / "run-status.json",
            json.dumps({"status": "INCOMPLETE"}, sort_keys=True) + "\n",
        )
        _write_artifact(
            artifact_dir / "task-input.json",
            task_text,
        )
        prompt = self.render_prompt(task)
        if _contains_secret(prompt):
            raise AdapterError("rendered prompt contains credential-like material")
        _write_artifact(artifact_dir / "prompt.md", prompt)
        invocation = {
            "role": self.manifest.name,
            "provider": self.manifest.provider,
            "model": self.manifest.model,
            "contract_model": self.manifest.contract_model,
            "worktree": str(worktree.resolve()),
            "fresh_session": True,
            "command": self.command("<prompt saved in prompt.md>", worktree),
        }
        _write_artifact(
            artifact_dir / "invocation.json",
            json.dumps(invocation, indent=2, sort_keys=True) + "\n",
        )
        verification = self._preflight(artifact_dir)
        reviewer_snapshot = (
            _git_snapshot(worktree)
            if self.manifest.name == "reviewer-secperf"
            else None
        )
        try:
            completed = _run_process(
                self.command(prompt, worktree), cwd=worktree, timeout=timeout
            )
        except ProcessTimeout as exc:
            _write_artifact(artifact_dir / "stdout.log", _redact(exc.stdout))
            _write_artifact(artifact_dir / "stderr.log", _redact(exc.stderr))
            raise AdapterError(f"Cursor execution failed: {exc}") from exc
        except AdapterError as exc:
            raise AdapterError(f"Cursor execution failed: {exc}") from exc
        stdout, stderr = completed.stdout, completed.stderr
        _write_artifact(artifact_dir / "stdout.log", _redact(stdout))
        _write_artifact(artifact_dir / "stderr.log", _redact(stderr))
        if completed.returncode != 0:
            raise AdapterError(f"Cursor exited with status {completed.returncode}")
        if (
            reviewer_snapshot is not None
            and _git_snapshot(worktree) != reviewer_snapshot
        ):
            raise AdapterError("read-only reviewer modified the assigned worktree")
        payload, envelope = _parse_cursor_output(stdout)
        if _contains_secret(json.dumps(payload, sort_keys=True)):
            raise AdapterError("Cursor result contains credential-like material")
        _write_artifact(
            artifact_dir / "cursor-envelope.json",
            json.dumps(envelope, indent=2, sort_keys=True) + "\n",
        )
        expected_schema = task.get("schema")
        if expected_schema != self.manifest.result_schema:
            raise AdapterError(
                f"task requested schema {expected_schema!r}, expected "
                f"{self.manifest.result_schema!r}"
            )
        try:
            validate_contract(self.manifest.result_schema, payload)
        except ContractError as exc:
            raise AdapterError(str(exc)) from exc
        if payload.get("role") != self.manifest.name:
            raise AdapterError("Cursor result role does not match adapter role")
        if payload.get("requested_model") != self.manifest.contract_model:
            raise AdapterError("Cursor result requested_model does not match manifest")
        _validate_task_bindings(task, payload)
        verification["result_claimed_model_unattested"] = payload.get("actual_model")
        _write_artifact(
            artifact_dir / "model-verification.json",
            json.dumps(verification, indent=2, sort_keys=True) + "\n",
        )
        _write_artifact(
            artifact_dir / "result.json",
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
        )
        _write_artifact(
            artifact_dir / "run-status.json",
            json.dumps({"status": "COMPLETE"}, sort_keys=True) + "\n",
        )
        return payload


def _parse_cursor_output(stdout: str) -> tuple[dict[str, Any], dict[str, Any]]:
    try:
        envelope = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise AdapterError("Cursor did not emit valid JSON") from exc
    if not isinstance(envelope, dict):
        raise AdapterError("Cursor JSON output must be an object")
    if envelope.get("type") != "result":
        raise AdapterError("Cursor JSON output is not a result envelope")
    if envelope.get("subtype") != "success" or envelope.get("is_error") is not False:
        raise AdapterError("Cursor result envelope reports failure")
    result = envelope.get("result")
    if isinstance(result, str):
        text = result.strip()
        if text.startswith("```json") and text.endswith("```"):
            text = text[7:-3].strip()
        try:
            payload = json.loads(text)
        except json.JSONDecodeError as exc:
            raise AdapterError("Cursor result text is not valid JSON") from exc
        if isinstance(payload, dict):
            return payload, envelope
    raise AdapterError("Cursor JSON envelope does not contain a result object")


def _run_process(
    command: Sequence[str], *, timeout: int, cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    with (
        tempfile.TemporaryFile(mode="w+t", encoding="utf-8") as stdout_file,
        tempfile.TemporaryFile(mode="w+t", encoding="utf-8") as stderr_file,
    ):
        process = subprocess.Popen(
            command,
            cwd=cwd,
            text=True,
            stdout=stdout_file,
            stderr=stderr_file,
            start_new_session=True,
            env=_sanitized_environment(),
        )
        try:
            process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as exc:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
            stdout = _read_bounded(stdout_file)
            stderr = _read_bounded(stderr_file)
            raise ProcessTimeout(
                f"process timed out after {timeout} seconds", stdout, stderr
            ) from exc
        stdout = _read_bounded(stdout_file)
        stderr = _read_bounded(stderr_file)
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def _write_artifact(path: Path, content: str) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    try:
        with temporary.open("x", encoding="utf-8") as handle:
            os.chmod(temporary, 0o600)
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        temporary.unlink(missing_ok=True)


def _read_bounded(handle: Any) -> str:
    handle.seek(0)
    content = handle.read(MAX_CAPTURE_BYTES + 1)
    if len(content.encode("utf-8")) > MAX_CAPTURE_BYTES:
        raise AdapterError(f"process output exceeded {MAX_CAPTURE_BYTES} bytes")
    return content


def _sanitized_environment() -> dict[str, str]:
    sensitive = re.compile(
        r"(?i)(token|secret|password|credential|api[_-]?key|private[_-]?key|"
        r"(?:^|_)key(?:$|_))"
    )
    return {
        key: value for key, value in os.environ.items() if not sensitive.search(key)
    }


def _contains_secret(text: str) -> bool:
    return bool(
        re.search(
            r"BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY|"
            r"gh[pousr]_[A-Za-z0-9]{20,}|"
            r"nsec1[023456789acdefghjklmnpqrstuvwxyz]{20,}|"
            r"sk-(?:proj-)?[A-Za-z0-9_-]{20,}|"
            r"AKIA[0-9A-Z]{16}|"
            r"AIza[0-9A-Za-z_-]{30,}|"
            r"xox[baprs]-[0-9A-Za-z-]{20,}",
            text,
        )
    )


def _redact(text: str) -> str:
    patterns = (
        r"BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY.*?END (?:RSA |EC |OPENSSH )?PRIVATE KEY",
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"nsec1[023456789acdefghjklmnpqrstuvwxyz]{20,}",
        r"sk-(?:proj-)?[A-Za-z0-9_-]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"AIza[0-9A-Za-z_-]{30,}",
        r"xox[baprs]-[0-9A-Za-z-]{20,}",
    )
    for pattern in patterns:
        text = re.sub(pattern, "[REDACTED]", text, flags=re.DOTALL)
    return text


def _git_snapshot(worktree: Path) -> tuple[str, str]:
    head = _run_process(["git", "rev-parse", "HEAD"], cwd=worktree, timeout=30)
    status = _run_process(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=worktree,
        timeout=30,
    )
    if head.returncode != 0 or status.returncode != 0:
        raise AdapterError("assigned reviewer worktree is not a readable Git worktree")
    return head.stdout.strip(), status.stdout


def _validate_task_input(
    role: str, result_schema: str, task: Mapping[str, Any]
) -> None:
    if task.get("schema") != result_schema:
        raise AdapterError(f"immutable task schema must be {result_schema}")
    for key in ("case_id", "task_id"):
        if not isinstance(task.get(key), str) or not task[key].strip():
            raise AdapterError(f"immutable task has invalid {key}")
    required = ["case_id", "task_id"]
    plan_version = task.get("plan_version")
    if plan_version is None and isinstance(task.get("plan"), Mapping):
        plan_version = task["plan"].get("version")
    if plan_version is None:
        raise AdapterError("immutable task is missing required bindings: plan_version")
    if type(plan_version) is not int or plan_version < 1:
        raise AdapterError("immutable task has invalid plan_version")
    if role == "reviewer-secperf":
        required.append("pr_number")
        if type(task.get("pr_number")) is not int or task["pr_number"] < 1:
            raise AdapterError("immutable task has invalid pr_number")
        expected_head = task.get("expected_head_sha")
        if expected_head is None:
            expected_head = task.get("head_sha")
        if expected_head is None:
            raise AdapterError("reviewer task requires expected_head_sha")
        if not isinstance(expected_head, str) or not re.fullmatch(
            r"[0-9a-f]{40}", expected_head
        ):
            raise AdapterError("reviewer task has invalid expected_head_sha")
    missing = [key for key in required if task.get(key) is None]
    if missing:
        raise AdapterError(
            f"immutable task is missing required bindings: {', '.join(missing)}"
        )


def _validate_task_bindings(
    task: Mapping[str, Any], payload: Mapping[str, Any]
) -> None:
    expected: dict[str, Any] = {
        "case_id": task.get("case_id"),
        "task_id": task.get("task_id"),
        "pr_number": task.get("pr_number"),
    }
    plan_version = task.get("plan_version")
    if plan_version is None and isinstance(task.get("plan"), Mapping):
        plan_version = task["plan"].get("version")
    expected["plan_version"] = plan_version
    expected_head = task.get("expected_head_sha")
    if expected_head is None:
        expected_head = task.get("head_sha")
    if expected_head is not None:
        expected["reviewed_head_sha"] = expected_head
    for field, value in expected.items():
        if value is not None and payload.get(field) != value:
            raise AdapterError(
                f"Cursor result {field} does not match immutable task input"
            )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run a direct Pip v2 Cursor role")
    parser.add_argument("role", choices=("builder-grok", "reviewer-secperf"))
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--task", type=Path, required=True)
    parser.add_argument("--worktree", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--executable", default="agent")
    parser.add_argument("--timeout", type=int, default=3600)
    args = parser.parse_args(argv)

    from .manifests import load_role_manifests

    roles = load_role_manifests(args.repo_root / "manifests" / "roles")
    try:
        task = json.loads(args.task.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise AdapterError(f"cannot load immutable task input: {exc}") from exc
    if not isinstance(task, dict):
        raise AdapterError("immutable task input must be a JSON object")
    adapter = CursorAdapter(
        args.repo_root,
        roles[args.role],
        executable=args.executable,
    )
    result = adapter.run(
        task,
        args.worktree,
        args.artifacts,
        timeout=args.timeout,
    )
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
