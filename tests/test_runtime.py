from __future__ import annotations

import json
import sqlite3
import stat
import subprocess
from pathlib import Path

import pytest

import pip_agent.cursor_adapter as cursor_module
import pip_agent.intake as intake_module
from pip_agent.bootstrap import (
    CLI_TOOLSET_CATALOG,
    BootstrapError,
    apply_profiles,
    install_shared_auth_links,
    install_skill_links,
    plan_profiles,
)
from pip_agent.case_store import CaseStore, CaseStoreError, ImmutableRunError
from pip_agent.contracts import validate_contract
from pip_agent.cursor_adapter import AdapterError, CursorAdapter
from pip_agent.intake import IntakeError, authorize_issue, planner_task_command
from pip_agent.manifests import load_role_manifests
from pip_agent.offline_fixture import FixtureFailure, run_offline_fixture
from pip_agent.state_machine import CaseState, TransitionError, transition

ROOT = Path(__file__).resolve().parents[1]


def test_role_manifests_define_three_hermes_and_two_direct_cursor_roles() -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")

    assert set(roles) == {
        "planner",
        "builder-grok",
        "reviewer-general",
        "reviewer-secperf",
        "final-reviewer",
    }
    assert {name for name, role in roles.items() if role.execution == "hermes"} == {
        "planner",
        "reviewer-general",
        "final-reviewer",
    }
    assert roles["planner"].model == "gpt-5.6-sol"
    assert roles["planner"].provider == "openai-codex"
    assert roles["planner"].reasoning == "xhigh"
    assert roles["reviewer-general"].reasoning == "high"
    assert roles["final-reviewer"].reasoning == "xhigh"
    assert roles["builder-grok"].model == "cursor-grok-4.6-high"
    assert roles["reviewer-secperf"].model == "kimi-k3-high"
    assert roles["builder-grok"].outer_reasoning_model is None
    assert roles["reviewer-secperf"].outer_reasoning_model is None
    for role in roles.values():
        assert role.skills == ("workflow-contract", role.name)
        assert role.fresh_session is True
    for role in roles.values():
        if role.execution == "hermes":
            assert {"skills", "kanban"}.issubset(role.toolsets)


def test_mdk_pilot_uses_pip_ok_but_remains_inert() -> None:
    config = json.loads((ROOT / "config/repositories/mdk.json").read_text())
    assert config["intake_label"] == "pip-ok"
    assert config["new_intake_enabled"] is False
    assert config["dispatch_enabled"] is False
    assert config["merge_mode"] == "shadow"
    assert config["autonomous_merge"] is False

    board = json.loads((ROOT / "config/boards/pip-mdk.json").read_text())
    assert board["slug"] == "pip-mdk"
    assert board["intake_label"] == "pip-ok"
    assert board["intake_enabled"] is False
    assert board["dispatch_enabled"] is False
    assert board["archived_until_activation"] is True
    assert board["merge_mode"] == "shadow"
    assert board["autonomous_merge"] is False


def test_intake_requires_trusted_pip_ok_label_event_and_open_issue() -> None:
    config = json.loads((ROOT / "config/repositories/mdk.json").read_text())
    issue = {
        "number": 1400,
        "state": "open",
        "title": "Fixture issue",
        "html_url": "https://github.com/marmot-protocol/mdk/issues/1400",
        "labels": [{"name": "pip-ok"}],
        "user": {"login": "someone"},
    }
    timeline = [
        {
            "event": "labeled",
            "label": {"name": "pip-ok"},
            "actor": {"login": "erskingardner"},
        }
    ]

    candidate = authorize_issue(issue, timeline, config)
    assert candidate.case_id == "mdk#1400"
    assert candidate.authorization_actor == "erskingardner"

    timeline[0]["actor"] = {"login": "untrusted"}
    with pytest.raises(IntakeError, match="trusted actor"):
        authorize_issue(issue, timeline, config)

    timeline.append({"event": "unlabeled", "label": {"name": "pip-ok"}})
    with pytest.raises(IntakeError, match="authorization event is missing"):
        authorize_issue(issue, timeline, config)


def test_intake_rejects_excluded_and_pull_request_numbers() -> None:
    config = json.loads((ROOT / "config/repositories/mdk.json").read_text())
    timeline = [
        {
            "event": "labeled",
            "label": {"name": "pip-ok"},
            "actor": {"login": "erskingardner"},
        }
    ]
    issue = {
        "number": 1384,
        "state": "open",
        "title": "Held",
        "html_url": "https://github.com/marmot-protocol/mdk/issues/1384",
        "labels": [{"name": "pip-ok"}],
        "user": {"login": "erskingardner"},
    }
    with pytest.raises(IntakeError, match="excluded"):
        authorize_issue(issue, timeline, config)
    issue["number"] = 1401
    issue["pull_request"] = {}
    with pytest.raises(IntakeError, match="pull request"):
        authorize_issue(issue, timeline, config)


def test_intake_uses_latest_label_lifecycle_beyond_first_page() -> None:
    config = json.loads((ROOT / "config/repositories/mdk.json").read_text())
    issue = {
        "number": 1402,
        "state": "open",
        "title": "Pagination fixture",
        "html_url": "https://github.com/marmot-protocol/mdk/issues/1402",
        "labels": [{"name": "pip-ok"}],
    }
    timeline: list[dict[str, object]] = [{"event": "commented"} for _ in range(101)]
    timeline.extend(
        [
            {
                "event": "labeled",
                "label": {"name": "pip-ok"},
                "actor": {"login": "erskingardner"},
            },
            {"event": "unlabeled", "label": {"name": "pip-ok"}},
            {
                "event": "labeled",
                "label": {"name": "pip-ok"},
                "actor": {"login": "untrusted"},
            },
        ]
    )
    with pytest.raises(IntakeError, match="trusted actor"):
        authorize_issue(issue, timeline, config)


def test_github_pagination_does_not_require_gh_slurp(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[list[str]] = []

    def fake_run(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
        calls.append(command)
        endpoint = command[2]
        if endpoint.endswith("page=1"):
            payload = [{"event": "commented", "id": index} for index in range(100)]
        elif endpoint.endswith("page=2"):
            payload = [{"event": "labeled", "id": 100}]
        else:
            raise AssertionError(endpoint)
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    monkeypatch.setattr(intake_module.subprocess, "run", fake_run)
    timeline = intake_module._gh_json(
        "repos/marmot-protocol/mdk/issues/1240/timeline?per_page=100",
        paginate=True,
    )

    assert len(timeline) == 101
    assert timeline[-1]["event"] == "labeled"
    assert [command[2] for command in calls] == [
        "repos/marmot-protocol/mdk/issues/1240/timeline?per_page=100&page=1",
        "repos/marmot-protocol/mdk/issues/1240/timeline?per_page=100&page=2",
    ]
    assert all("--slurp" not in command for command in calls)


def test_planner_task_routing_is_deduplicated_and_force_loads_both_skills() -> None:
    config = json.loads((ROOT / "config/repositories/mdk.json").read_text())
    config["new_intake_enabled"] = True
    config["dispatch_enabled"] = True
    config["canary_issue_number"] = 1400
    issue = {
        "number": 1400,
        "state": "open",
        "title": "Fixture issue",
        "html_url": "https://github.com/marmot-protocol/mdk/issues/1400",
        "labels": [{"name": "pip-ok"}],
        "user": {"login": "erskingardner"},
    }
    candidate = authorize_issue(
        issue,
        [
            {
                "event": "labeled",
                "label": {"name": "pip-ok"},
                "actor": {"login": "erskingardner"},
            }
        ],
        config,
    )
    command = planner_task_command(candidate, config)
    assert command[:4] == ["hermes", "kanban", "--board", "pip-mdk"]
    assert command.count("--skill") == 2
    assert "workflow-contract" in command
    assert "planner" in command
    assert "pip-v2:marmot-protocol/mdk:1400:plan-v1" in command

    config["canary_issue_number"] = 1401
    with pytest.raises(IntakeError, match="exact one-issue canary"):
        planner_task_command(candidate, config)
    config["canary_issue_number"] = 1400

    config["new_intake_enabled"] = False
    with pytest.raises(IntakeError, match="disabled"):
        planner_task_command(candidate, config)
    config["new_intake_enabled"] = True
    config["dispatch_enabled"] = False
    with pytest.raises(IntakeError, match="dispatch is disabled"):
        planner_task_command(candidate, config)


def test_profile_plan_contains_only_hermes_native_roles() -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    actions = plan_profiles(roles)

    assert [action.profile for action in actions] == [
        "final-reviewer",
        "planner",
        "reviewer-general",
    ]
    assert all(action.model == "gpt-5.6-sol" for action in actions)
    assert {action.reasoning for action in actions} == {"high", "xhigh"}


def test_skill_links_are_idempotent_and_reject_foreign_targets(tmp_path: Path) -> None:
    profile_home = tmp_path / "profiles" / "planner"
    profile_home.mkdir(parents=True)

    installed = install_skill_links(
        profile_home=profile_home,
        repo_root=ROOT,
        role_name="planner",
    )
    assert {path.name for path in installed} == {"planner", "workflow-contract"}
    assert all(path.is_symlink() for path in installed)

    repeated = install_skill_links(
        profile_home=profile_home,
        repo_root=ROOT,
        role_name="planner",
    )
    assert repeated == installed

    installed[0].unlink()
    installed[0].symlink_to(tmp_path / "foreign")
    with pytest.raises(BootstrapError, match="foreign symlink"):
        install_skill_links(
            profile_home=profile_home,
            repo_root=ROOT,
            role_name="planner",
        )


def test_shared_auth_links_use_one_lock_and_reject_copied_credentials(
    tmp_path: Path,
) -> None:
    shared_home = tmp_path / "shared"
    profile_home = tmp_path / "profiles/planner"
    shared_home.mkdir()
    profile_home.mkdir(parents=True)
    (shared_home / "auth.json").write_text("{}\n")
    (shared_home / "auth.lock").write_text("")

    links = install_shared_auth_links(profile_home, shared_home)
    assert [link.name for link in links] == ["auth.json", "auth.lock"]
    assert all(link.is_symlink() for link in links)
    assert install_shared_auth_links(profile_home, shared_home) == links

    links[0].unlink()
    links[0].write_text("copied credential state")
    with pytest.raises(BootstrapError, match="existing auth path"):
        install_shared_auth_links(profile_home, shared_home)


def test_profile_apply_scopes_every_config_write_to_named_profile(
    tmp_path: Path,
) -> None:
    hermes_home = tmp_path / ".hermes"
    (hermes_home / "auth.json").parent.mkdir(parents=True)
    (hermes_home / "auth.json").write_text("{}\n")
    (hermes_home / "auth.lock").write_text("")
    commands: list[list[str]] = []
    environments: list[dict[str, str]] = []

    def capture(command: object, env: object = None) -> None:
        materialized = list(command)  # type: ignore[arg-type]
        commands.append(materialized)
        environments.append(dict(env))  # type: ignore[arg-type]
        if materialized[1:3] == ["profile", "create"]:
            (hermes_home / "profiles" / materialized[3]).mkdir(parents=True)

    apply_profiles(repo_root=ROOT, hermes_home=hermes_home, runner=capture)

    config_commands = [command for command in commands if "config" in command]
    assert config_commands
    assert all(command[1] == "-p" for command in config_commands)
    assert {command[2] for command in config_commands} == {
        "planner",
        "reviewer-general",
        "final-reviewer",
    }
    assert all(
        "model.reasoning_effort" in command or "reasoning_effort" not in command
        for command in config_commands
    )
    tool_commands = [command for command in commands if "tools" in command]
    assert sum("disable" in command for command in tool_commands) == (
        len(CLI_TOOLSET_CATALOG) * 3
    )
    assert sum("enable" in command for command in tool_commands) == 15
    assert sum(command[-2:] == ["fallback", "clear"] for command in commands) == 3
    assert environments
    assert all(env["HERMES_HOME"] == str(hermes_home.resolve()) for env in environments)


def _write_fake_cursor(
    path: Path,
    payload: dict,
    *,
    models: tuple[str, ...] = (),
    mutate_worktree: bool = False,
) -> None:
    advertised = models or ("cursor-grok-4.6-high", "kimi-k3-high")
    script = (
        "#!/usr/bin/env python3\n"
        "import json, sys\n"
        "from pathlib import Path\n"
        f"models = {advertised!r}\n"
        "if len(sys.argv) > 1 and sys.argv[1] == 'models':\n"
        "    print('Available models')\n"
        "    for model in models: print(f'{model} - fixture')\n"
        "elif len(sys.argv) > 1 and sys.argv[1] == 'status':\n"
        "    print(json.dumps({'status': 'authenticated', 'isAuthenticated': True}))\n"
        "else:\n"
        + ("    Path('tracked.txt').write_text('mutated')\n" if mutate_worktree else "")
        + f"    payload = {payload!r}\n"
        "    print(json.dumps({'type': 'result', 'subtype': 'success', "
        "'is_error': False, 'result': json.dumps(payload), 'usage': {}}))\n"
    )
    path.write_text(script)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


def _builder_payload(model: str = "cursor/cursor-grok-4.6-high") -> dict:
    head = "b" * 40
    return {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#fixture",
        "task_id": "build-r1",
        "role": "builder-grok",
        "outcome": "REVIEW_READY",
        "requested_model": "cursor/cursor-grok-4.6-high",
        "actual_model": model,
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "plan_version": 1,
        "build_round": 1,
        "pr_number": 1,
        "head_sha": head,
        "ci_head_sha": head,
        "github_ci_green": True,
        "local_checks": ["fixture"],
        "finding_resolutions": [],
        "evidence": {},
    }


def test_cursor_adapter_renders_both_skills_and_validates_fixture_output(
    tmp_path: Path,
) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, _builder_payload())
    task = {
        "case_id": "mdk#fixture",
        "task_id": "build-r1",
        "schema": "builder-result",
        "plan": {"version": 1, "summary": "fixture only"},
    }
    (tmp_path / "worktree").mkdir()

    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)
    result = adapter.run(task, tmp_path / "worktree", tmp_path / "artifacts")

    assert result["outcome"] == "REVIEW_READY"
    prompt = (tmp_path / "artifacts/prompt.md").read_text()
    assert "# Pip v2 Workflow Contract" in prompt
    assert "# Builder Grok" in prompt
    invocation = json.loads((tmp_path / "artifacts/invocation.json").read_text())
    assert invocation["model"] == "cursor-grok-4.6-high"
    assert invocation["worktree"] == str((tmp_path / "worktree").resolve())
    assert "--sandbox" in invocation["command"]
    assert "--no-mcps" not in invocation["command"]
    verification = json.loads(
        (tmp_path / "artifacts/model-verification.json").read_text()
    )
    assert verification["requested_model"] == "cursor-grok-4.6-high"
    assert verification["exact_model_advertised"] is True
    assert verification["cli_reports_actual_model"] is False
    assert json.loads((tmp_path / "artifacts/run-status.json").read_text()) == {
        "status": "COMPLETE"
    }
    assert stat.S_IMODE((tmp_path / "artifacts").stat().st_mode) == 0o700
    assert all(
        stat.S_IMODE(path.stat().st_mode) == 0o600
        for path in (tmp_path / "artifacts").iterdir()
    )


def test_cursor_adapter_fails_closed_on_wrong_model(tmp_path: Path) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, _builder_payload("cursor/auto"))
    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)
    (tmp_path / "worktree").mkdir()

    with pytest.raises(AdapterError, match="model substitution"):
        adapter.run(
            {
                "case_id": "mdk#fixture",
                "task_id": "build-r1",
                "schema": "builder-result",
                "plan_version": 1,
            },
            tmp_path / "worktree",
            tmp_path / "artifacts",
        )


def test_cursor_adapter_rejects_unadvertised_pinned_model(tmp_path: Path) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(
        executable,
        _builder_payload(),
        models=("kimi-k3-high",),
    )
    (tmp_path / "worktree").mkdir()
    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)
    with pytest.raises(AdapterError, match="not advertised"):
        adapter.run(
            {
                "case_id": "mdk#fixture",
                "task_id": "build-r1",
                "schema": "builder-result",
                "plan_version": 1,
            },
            tmp_path / "worktree",
            tmp_path / "artifacts",
        )


def test_cursor_adapter_binds_result_to_immutable_task_input(tmp_path: Path) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    payload = _builder_payload()
    payload["case_id"] = "wrong#999"
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, payload)
    (tmp_path / "worktree").mkdir()
    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)

    with pytest.raises(AdapterError, match="case_id"):
        adapter.run(
            {
                "case_id": "mdk#fixture",
                "task_id": "build-r1",
                "schema": "builder-result",
                "plan_version": 1,
            },
            tmp_path / "worktree",
            tmp_path / "artifacts",
        )


def test_cursor_adapter_rejects_missing_task_bindings_before_execution(
    tmp_path: Path,
) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, _builder_payload())
    (tmp_path / "worktree").mkdir()
    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)
    with pytest.raises(AdapterError, match="invalid case_id"):
        adapter.run(
            {"schema": "builder-result"},
            tmp_path / "worktree",
            tmp_path / "artifacts",
        )
    assert not (tmp_path / "artifacts").exists()


def test_cursor_adapter_sanitizes_credential_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("GH_TOKEN", "canary")
    monkeypatch.setenv("AWS_ACCESS_KEY_ID", "canary")
    monkeypatch.setenv("SAFE_FIXTURE", "kept")
    sanitized = cursor_module._sanitized_environment()
    assert "GH_TOKEN" not in sanitized
    assert "AWS_ACCESS_KEY_ID" not in sanitized
    assert sanitized["SAFE_FIXTURE"] == "kept"


@pytest.mark.parametrize(
    "canary",
    (
        "nsec1" + "q" * 32,
        "sk-proj-" + "A" * 24,
        "AKIA" + "A" * 16,
    ),
)
def test_cursor_adapter_rejects_secret_canary_before_persisting(
    tmp_path: Path, canary: str
) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, _builder_payload())
    (tmp_path / "worktree").mkdir()
    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)
    with pytest.raises(AdapterError, match="credential-like"):
        adapter.run(
            {
                "case_id": "mdk#fixture",
                "task_id": "build-r1",
                "schema": "builder-result",
                "plan_version": 1,
                "canary": canary,
            },
            tmp_path / "worktree",
            tmp_path / "artifacts",
        )
    assert not (tmp_path / "artifacts").exists()


def test_cursor_binding_uses_head_fallback_when_expected_head_is_null() -> None:
    task = {
        "case_id": "mdk#fixture",
        "task_id": "review-r1",
        "plan_version": 1,
        "pr_number": 77,
        "expected_head_sha": None,
        "head_sha": "a" * 40,
    }
    payload = {
        "case_id": "mdk#fixture",
        "task_id": "review-r1",
        "plan_version": 1,
        "pr_number": 77,
        "reviewed_head_sha": "b" * 40,
    }
    with pytest.raises(AdapterError, match="reviewed_head_sha"):
        cursor_module._validate_task_bindings(task, payload)
    with pytest.raises(AdapterError, match="invalid expected_head_sha"):
        cursor_module._validate_task_input(
            "reviewer-secperf",
            "review-result",
            {**task, "schema": "review-result", "head_sha": "bad"},
        )


def test_cursor_adapter_bounds_captured_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    payload = _builder_payload()
    payload["evidence"] = {"oversized": "x" * 3_000}
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, payload)
    (tmp_path / "worktree").mkdir()
    monkeypatch.setattr(cursor_module, "MAX_CAPTURE_BYTES", 1_024)
    adapter = CursorAdapter(ROOT, roles["builder-grok"], executable=executable)
    with pytest.raises(AdapterError, match="output exceeded"):
        adapter.run(
            {
                "case_id": "mdk#fixture",
                "task_id": "build-r1",
                "schema": "builder-result",
                "plan_version": 1,
            },
            tmp_path / "worktree",
            tmp_path / "artifacts",
        )
    assert json.loads((tmp_path / "artifacts/run-status.json").read_text()) == {
        "status": "INCOMPLETE"
    }


def test_cursor_reviewer_fails_if_worktree_changes(tmp_path: Path) -> None:
    roles = load_role_manifests(ROOT / "manifests" / "roles")
    executable = tmp_path / "cursor-agent"
    _write_fake_cursor(executable, {}, mutate_worktree=True)
    worktree = tmp_path / "worktree"
    worktree.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=worktree, check=True)
    (worktree / "tracked.txt").write_text("original")
    subprocess.run(["git", "add", "tracked.txt"], cwd=worktree, check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Pip Fixture",
            "-c",
            "user.email=pip-fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
        cwd=worktree,
        check=True,
    )
    adapter = CursorAdapter(ROOT, roles["reviewer-secperf"], executable=executable)
    with pytest.raises(AdapterError, match="modified"):
        adapter.run(
            {
                "case_id": "mdk#fixture",
                "task_id": "review-secperf-r1",
                "schema": "review-result",
                "plan_version": 1,
                "pr_number": 77,
                "expected_head_sha": "a" * 40,
            },
            worktree,
            tmp_path / "artifacts",
        )


def test_case_store_preserves_permanent_case_and_immutable_runs(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "cases.db")
    assert (tmp_path / "cases.db").stat().st_mode & 0o777 == 0o600
    store.create_case("mdk#1", "marmot-protocol/mdk", 1, "pip-ok")
    assert store.ensure_case("mdk#1", "marmot-protocol/mdk", 1, "pip-ok") is False
    with pytest.raises(CaseStoreError, match="already exists"):
        store.create_case("mdk#1", "marmot-protocol/mdk", 1, "pip-ok")
    store.append_run(
        "mdk#1",
        "plan-v1",
        "planner",
        1,
        {
            "case_id": "mdk#1",
            "task_id": "plan-v1",
            "role": "planner",
            "outcome": "PROCEED",
        },
    )
    store.record_plan_version("mdk#1", 0, 1)

    with pytest.raises(ImmutableRunError):
        store.append_run(
            "mdk#1",
            "plan-v1",
            "planner",
            1,
            {
                "case_id": "mdk#1",
                "task_id": "plan-v1",
                "role": "planner",
                "outcome": "DIFFERENT",
            },
        )

    with pytest.raises(CaseStoreError, match="case_id"):
        store.append_run(
            "mdk#1",
            "bad-r1",
            "planner",
            2,
            {"case_id": "other#9", "task_id": "bad-r1", "role": "planner"},
        )

    store.append_run(
        "mdk#1",
        "build-r1",
        "builder-grok",
        1,
        {
            "case_id": "mdk#1",
            "task_id": "build-r1",
            "role": "builder-grok",
            "outcome": "REVIEW_READY",
        },
    )
    case = store.get_case("mdk#1")
    assert case["state"] == "PLANNING"
    validate_contract("case", case)
    store.set_state("mdk#1", "PLANNING", "PROCEED", "fixture-transition")
    store.bind_pr_head("mdk#1", 77, "a" * 40)
    with pytest.raises(CaseStoreError, match="stale PR head"):
        store.bind_pr_head("mdk#1", 77, "b" * 40)
    with pytest.raises(CaseStoreError, match="stale state transition"):
        store.set_state("mdk#1", "PLANNING", "PROCEED", "stale-transition")
    with pytest.raises(CaseStoreError, match="invalid transition"):
        store.set_state("mdk#1", "BUILDING", "MERGE", "invalid-transition")
    assert store.get_case("mdk#1")["state"] == "BUILDING"
    assert store.get_case("mdk#1")["plan_version"] == 1
    assert store.get_case("mdk#1")["pr_number"] == 77
    assert store.get_case("mdk#1")["current_pr_head"] == "a" * 40
    assert [run["task_id"] for run in store.list_runs("mdk#1")] == [
        "plan-v1",
        "build-r1",
    ]

    with sqlite3.connect(tmp_path / "cases.db") as db:
        with pytest.raises(sqlite3.IntegrityError, match="immutable run"):
            db.execute("UPDATE runs SET role = 'tampered' WHERE task_id = 'plan-v1'")
        with pytest.raises(sqlite3.IntegrityError, match="immutable run"):
            db.execute("DELETE FROM runs WHERE task_id = 'plan-v1'")
        with pytest.raises(sqlite3.IntegrityError, match="append-only"):
            db.execute("DELETE FROM events WHERE case_id = 'mdk#1'")
        with pytest.raises(sqlite3.IntegrityError, match="permanent case"):
            db.execute("UPDATE cases SET issue_number = 2 WHERE case_id = 'mdk#1'")
        with pytest.raises(sqlite3.IntegrityError, match="permanent cases"):
            db.execute("DELETE FROM cases WHERE case_id = 'mdk#1'")
        with pytest.raises(sqlite3.DatabaseError):
            db.execute(
                """
                INSERT INTO cases(
                    case_id, repository, issue_number, intake_label, state,
                    created_at, updated_at
                ) VALUES (
                    'mdk#999', 'marmot-protocol/mdk', 999, 'pip-ok',
                    'PLANNING', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
                )
                """
            )
        with pytest.raises(sqlite3.DatabaseError):
            db.execute(
                """
                INSERT INTO runs(
                    task_id, case_id, role, ordinal, payload_json,
                    payload_sha256, created_at
                ) VALUES (
                    'forged-run', 'mdk#1', 'planner', 99, '{}',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    '2026-01-01T00:00:00Z'
                )
                """
            )
        with pytest.raises(sqlite3.DatabaseError):
            db.execute("UPDATE cases SET state = 'COMPLETED' WHERE case_id = 'mdk#1'")
        with pytest.raises(sqlite3.DatabaseError):
            db.execute("UPDATE cases SET remediation_round = 1 WHERE case_id = 'mdk#1'")
        with pytest.raises(sqlite3.DatabaseError):
            db.execute("UPDATE cases SET plan_version = 2 WHERE case_id = 'mdk#1'")
        with pytest.raises(sqlite3.DatabaseError):
            db.execute(
                "UPDATE cases SET current_pr_head = ? WHERE case_id = 'mdk#1'",
                ("b" * 40,),
            )
        with pytest.raises(sqlite3.DatabaseError):
            db.execute("UPDATE cases SET updated_at = '2099-01-01T00:00:00Z'")
        with pytest.raises(sqlite3.DatabaseError):
            db.execute(
                """
                INSERT INTO events(case_id, from_state, to_state, reason, created_at)
                VALUES ('mdk#1', 'BUILDING', 'REVIEWING', 'forged',
                        '2099-01-01T00:00:00Z')
                """
            )


def test_case_store_durably_bounds_remediation_rounds(tmp_path: Path) -> None:
    store = CaseStore(tmp_path / "bounded.db")
    store.create_case("mdk#2", "marmot-protocol/mdk", 2, "pip-ok")
    state = store.set_state("mdk#2", "PLANNING", "PROCEED", "planned")
    for round_number in range(4):
        state = store.set_state(
            "mdk#2", state.value, "REVIEW_READY", f"review {round_number}"
        )
        state = store.set_state(
            "mdk#2", state.value, "REQUEST_CHANGES", f"finding {round_number}"
        )
    assert state is CaseState.ESCALATED
    case = store.get_case("mdk#2")
    assert case["state"] == "ESCALATED"
    assert case["remediation_round"] == 3


def test_case_store_rejects_symlinked_database(tmp_path: Path) -> None:
    target = tmp_path / "target.db"
    target.touch()
    link = tmp_path / "linked.db"
    link.symlink_to(target)
    with pytest.raises(CaseStoreError, match="symlink"):
        CaseStore(link)


@pytest.mark.parametrize(
    ("outcome", "expected_state"),
    (
        ("RETURN_TO_BUILD", CaseState.BUILDING),
        ("RETURN_TO_REVIEW", CaseState.REVIEWING),
        ("RETURN_TO_PLANNING", CaseState.PLANNING),
        ("WAIT_FOR_ISSUE_CREATOR", CaseState.WAITING_HUMAN),
    ),
)
def test_case_store_counts_final_review_rework(
    tmp_path: Path, outcome: str, expected_state: CaseState
) -> None:
    store = CaseStore(tmp_path / "final-rework.db")
    store.create_case("mdk#3", "marmot-protocol/mdk", 3, "pip-ok")
    store.record_plan_version("mdk#3", 0, 1)
    state = store.set_state("mdk#3", "PLANNING", "PROCEED", "planned")
    state = store.set_state("mdk#3", state.value, "REVIEW_READY", "built")
    state = store.set_state("mdk#3", state.value, "REVIEWS_APPROVED", "approved")
    state = store.set_state("mdk#3", state.value, outcome, "final finding")
    assert state is expected_state
    assert store.get_case("mdk#3")["remediation_round"] == 1


def test_state_machine_routes_happy_path_and_rejects_stale_events() -> None:
    assert transition(CaseState.PLANNING, "PROCEED") is CaseState.BUILDING
    assert (
        transition(CaseState.PLANNING, "ROOT_CAUSE_DIFFERENT_SCOPE")
        is CaseState.WAITING_HUMAN
    )
    assert transition(CaseState.BUILDING, "REVIEW_READY") is CaseState.REVIEWING
    assert transition(CaseState.BUILDING, "RETURN_TO_PLANNING") is CaseState.PLANNING
    assert transition(CaseState.BUILDING, "ABANDON") is CaseState.ABANDONED
    assert transition(CaseState.REVIEWING, "REVIEWS_APPROVED") is CaseState.FINAL_REVIEW
    assert transition(CaseState.FINAL_REVIEW, "MERGE") is CaseState.SHADOW_READY

    with pytest.raises(TransitionError):
        transition(CaseState.PLANNING, "MERGE")
    with pytest.raises(TransitionError, match="terminal"):
        transition(CaseState.COMPLETED, "BLOCKED_UNEXPECTED_MODEL")


def test_state_machine_bounds_remediation_loops() -> None:
    assert (
        transition(
            CaseState.REVIEWING, "REQUEST_CHANGES", remediation_round=2, max_rounds=3
        )
        is CaseState.BUILDING
    )
    assert (
        transition(
            CaseState.REVIEWING, "REQUEST_CHANGES", remediation_round=3, max_rounds=3
        )
        is CaseState.ESCALATED
    )


def test_offline_end_to_end_fixture_reaches_shadow_ready_without_merging(
    tmp_path: Path,
) -> None:
    result = run_offline_fixture(tmp_path / "fixture.db")

    assert result["state"] == "SHADOW_READY"
    assert result["merge_performed"] is False
    assert result["shadow_mode"] is True
    assert result["run_ids"] == [
        "plan-v1",
        "build-r1",
        "reviewer-general-r1",
        "reviewer-secperf-r1",
        "final-review-r1",
    ]


@pytest.mark.parametrize(
    "fault",
    ["wrong-model", "stale-head", "red-ci", "malformed-result"],
)
def test_offline_fixture_faults_fail_closed(tmp_path: Path, fault: str) -> None:
    with pytest.raises(FixtureFailure):
        run_offline_fixture(tmp_path / f"{fault}.db", fault)  # type: ignore[arg-type]
