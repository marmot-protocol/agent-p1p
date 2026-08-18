from __future__ import annotations

import json
import os
import subprocess
import sys
import tomllib
import zipfile
from pathlib import Path

import pytest

from pip_agent.contracts import (
    ContractError,
    assert_exact_head_evidence,
    load_schema,
    validate_contract,
)
from pip_agent.control_plane import (
    render_decision_service_unit,
    render_decision_timer_unit,
    render_service_unit,
)
from pip_agent.route_consumer import (
    render_route_consumer_service,
    render_route_consumer_timer,
)

ROOT = Path(__file__).resolve().parents[1]


def route_binding() -> dict[str, object]:
    return {
        "route_id": "decision-" + "d" * 64,
        "comment_id": 2,
        "evidence_body_sha256": "e" * 64,
        "planner_comment_id": 1,
        "planner_body_sha256": "f" * 64,
        "planned_base_sha": "c" * 40,
    }


def review_result(role: str, head: str, *, outcome: str = "APPROVE") -> dict:
    model = (
        "openai-codex/gpt-5.6-sol"
        if role == "reviewer-general"
        else "cursor/kimi-k3-high"
    )
    return {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": f"{role}-r1",
        "role": role,
        "outcome": outcome,
        "requested_model": model,
        "actual_model": model,
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "review_round": 1,
        "plan_version": 1,
        "pr_number": 1400,
        "reviewed_head_sha": head,
        "blocking_findings": [],
        "finding_confirmations": [],
        "suggestions": [],
        "evidence": {},
    }


def builder_result(head: str, *, resolutions: list[dict] | None = None) -> dict:
    return {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "builder-grok-r2",
        "role": "builder-grok",
        "outcome": "REVIEW_READY",
        "requested_model": "cursor/cursor-grok-4.6-high",
        "actual_model": "cursor/cursor-grok-4.6-high",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "plan_version": 1,
        "build_round": 2,
        "pr_number": 1400,
        "head_sha": head,
        "ci_head_sha": head,
        "github_ci_green": True,
        "local_checks": ["uv run pytest"],
        "finding_resolutions": resolutions or [],
        "evidence": {},
    }


def join_evidence(head: str) -> dict:
    return {
        "general_review": review_result("reviewer-general", head),
        "secperf_review": review_result("reviewer-secperf", head),
        "builder_result": builder_result(head),
        "mandatory_findings": [],
        "ci": {
            "head_sha": head,
            "required_checks_green": True,
            "completed": True,
            "hollow": False,
            "rate_limited": False,
        },
        "open_blocking_threads": 0,
        "pip_owned": True,
        "mergeable": True,
        "issue_authorization_valid": True,
    }


def assert_join(head: str, evidence: dict) -> None:
    assert_exact_head_evidence("mdk#1400", 1400, 1, head, evidence)


def test_required_repository_scaffold_exists() -> None:
    required = [
        ".github/workflows/ci.yml",
        "README.md",
        "docs/pip-v2-architecture-plan.md",
        "config/repositories/mdk.json",
        "schemas/case.schema.json",
        "schemas/common-result.schema.json",
        "schemas/planner-result.schema.json",
        "schemas/builder-result.schema.json",
        "schemas/review-result.schema.json",
        "schemas/final-result.schema.json",
        "skills/shared/workflow-contract/SKILL.md",
        "skills/planner/SKILL.md",
        "skills/builder-grok/SKILL.md",
        "skills/reviewer-general/SKILL.md",
        "skills/reviewer-secperf/SKILL.md",
        "skills/final-reviewer/SKILL.md",
    ]

    missing = [path for path in required if not (ROOT / path).is_file()]
    assert missing == []


def test_mdk_is_configured_as_shadow_merge_pilot() -> None:
    config = json.loads((ROOT / "config/repositories/mdk.json").read_text())

    assert config["repository"] == "marmot-protocol/mdk"
    assert config["board"] == "pip-mdk"
    assert config["workflow_version"] == 2
    assert config["merge_mode"] == "shadow"
    assert config["legacy_existing_work_stays_on_legacy_board"] is True
    assert config["models"] == {
        "planner": "openai-codex/gpt-5.6-sol",
        "builder-grok": "cursor/cursor-grok-4.6-high",
        "reviewer-general": "openai-codex/gpt-5.6-sol",
        "reviewer-secperf": "cursor/kimi-k3-high",
        "final-reviewer": "openai-codex/gpt-5.6-sol",
    }


def test_all_contract_schemas_load() -> None:
    for name in (
        "case",
        "common-result",
        "planner-result",
        "builder-result",
        "review-result",
        "final-result",
    ):
        schema = load_schema(name)
        assert schema["$schema"].endswith("2020-12/schema")


def test_validate_contract_accepts_valid_review_result() -> None:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "reviewer-general-r1",
        "role": "reviewer-general",
        "outcome": "APPROVE",
        "requested_model": "openai-codex/gpt-5.6-sol",
        "actual_model": "openai-codex/gpt-5.6-sol",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "review_round": 1,
        "plan_version": 1,
        "pr_number": 1400,
        "reviewed_head_sha": "b" * 40,
        "blocking_findings": [],
        "finding_confirmations": [],
        "suggestions": [],
        "evidence": {
            "review_url": "https://github.com/marmot-protocol/mdk/pull/1400#issuecomment-1"
        },
    }

    validate_contract("review-result", result)


def test_validate_contract_rejects_model_substitution() -> None:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "reviewer-secperf-r1",
        "role": "reviewer-secperf",
        "outcome": "APPROVE",
        "requested_model": "cursor/kimi-k3-high",
        "actual_model": "cursor/auto",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "review_round": 1,
        "plan_version": 1,
        "pr_number": 1400,
        "reviewed_head_sha": "b" * 40,
        "blocking_findings": [],
        "finding_confirmations": [],
        "suggestions": [],
        "evidence": {},
    }

    with pytest.raises(ContractError, match="model substitution"):
        validate_contract("review-result", result)


def test_model_mismatch_can_be_recorded_as_structured_block() -> None:
    result = review_result("reviewer-secperf", "b" * 40)
    result.update(
        outcome="BLOCKED_UNEXPECTED_MODEL",
        actual_model="cursor/auto",
    )

    validate_contract("review-result", result)


def test_schema_name_cannot_escape_registry() -> None:
    with pytest.raises(ContractError, match="unknown schema"):
        load_schema("../../tmp/attacker")


def test_review_contract_rejects_invalid_role_timestamp_and_approval_with_blocker() -> (
    None
):
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "review-r1",
        "role": "not-a-reviewer",
        "outcome": "APPROVE",
        "requested_model": "openai-codex/gpt-5.6-sol",
        "actual_model": "openai-codex/gpt-5.6-sol",
        "skills_repository_commit": "a" * 40,
        "started_at": "not-a-timestamp",
        "completed_at": "also-not-a-timestamp",
        "review_round": 1,
        "plan_version": 1,
        "pr_number": 1400,
        "reviewed_head_sha": "b" * 40,
        "blocking_findings": [{"id": "GENERAL-R1-001", "summary": "broken"}],
        "suggestions": [],
        "evidence": {},
    }

    with pytest.raises(ContractError):
        validate_contract("review-result", result)


def test_builder_contract_allows_safe_return_to_planning_without_pr() -> None:
    result = builder_result("b" * 40)
    result["outcome"] = "RETURN_TO_PLANNING"
    for key in ("pr_number", "head_sha", "ci_head_sha", "github_ci_green"):
        result.pop(key)

    validate_contract("builder-result", result)


def test_builder_contract_requires_pr_and_exact_head_fields_when_review_ready() -> None:
    result = builder_result("b" * 40)
    result.pop("pr_number")

    with pytest.raises(ContractError):
        validate_contract("builder-result", result)


def test_builder_contract_rejects_green_ci_for_stale_head() -> None:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "build-r1",
        "role": "builder-grok",
        "outcome": "REVIEW_READY",
        "requested_model": "cursor/cursor-grok-4.6-high",
        "actual_model": "cursor/cursor-grok-4.6-high",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "plan_version": 1,
        "build_round": 1,
        "pr_number": 1400,
        "head_sha": "b" * 40,
        "ci_head_sha": "c" * 40,
        "github_ci_green": True,
        "local_checks": ["uv run pytest"],
        "finding_resolutions": [],
        "evidence": {},
    }

    with pytest.raises(ContractError, match="CI head does not match"):
        validate_contract("builder-result", result)


def test_builder_contract_rejects_review_ready_with_red_ci() -> None:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "build-r1",
        "role": "builder-grok",
        "outcome": "REVIEW_READY",
        "requested_model": "cursor/cursor-grok-4.6-high",
        "actual_model": "cursor/cursor-grok-4.6-high",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "plan_version": 1,
        "build_round": 1,
        "pr_number": 1400,
        "head_sha": "b" * 40,
        "ci_head_sha": "b" * 40,
        "github_ci_green": False,
        "local_checks": ["uv run pytest"],
        "finding_resolutions": [],
        "evidence": {},
    }

    with pytest.raises(ContractError, match="requires green CI"):
        validate_contract("builder-result", result)


def test_common_result_rejects_role_outcome_mismatch() -> None:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "build-r1",
        "role": "builder-grok",
        "outcome": "ARBITRARY",
        "requested_model": "cursor/cursor-grok-4.6-high",
        "actual_model": "cursor/cursor-grok-4.6-high",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "evidence": {},
    }

    with pytest.raises(ContractError):
        validate_contract("common-result", result)


def test_review_finding_rejects_unstructured_resolution_data() -> None:
    result = {
        "schema_version": 1,
        "workflow_version": 2,
        "case_id": "mdk#1400",
        **route_binding(),
        "task_id": "review-r1",
        "role": "reviewer-general",
        "outcome": "REQUEST_CHANGES",
        "requested_model": "openai-codex/gpt-5.6-sol",
        "actual_model": "openai-codex/gpt-5.6-sol",
        "skills_repository_commit": "a" * 40,
        "started_at": "2026-08-15T12:00:00Z",
        "completed_at": "2026-08-15T12:10:00Z",
        "review_round": 1,
        "plan_version": 1,
        "pr_number": 1400,
        "reviewed_head_sha": "b" * 40,
        "blocking_findings": [
            {
                "id": "GENERAL-R1-001",
                "summary": "broken",
                "resolved": True,
                "resolution_commit": "not-a-sha",
            }
        ],
        "finding_confirmations": [],
        "suggestions": [],
        "evidence": {},
    }

    with pytest.raises(ContractError):
        validate_contract("review-result", result)


def test_join_evidence_rejects_stale_review_sha() -> None:
    head = "c" * 40
    evidence = join_evidence(head)
    evidence["secperf_review"]["reviewed_head_sha"] = "d" * 40

    with pytest.raises(ContractError, match="reviewer-secperf reviewed stale head"):
        assert_join(head, evidence)


def test_join_evidence_rejects_empty_head_and_missing_fail_closed_fields() -> None:
    evidence = join_evidence("e" * 40)

    with pytest.raises(ContractError, match="valid 40-character"):
        assert_join("", evidence)


def test_join_evidence_requires_ownership_complete_ci_and_resolved_findings() -> None:
    head = "e" * 40
    evidence = join_evidence(head)
    evidence["pip_owned"] = False

    with pytest.raises(ContractError, match="Pip-owned"):
        assert_join(head, evidence)


def test_join_evidence_rejects_boolean_counter() -> None:
    head = "e" * 40
    evidence = join_evidence(head)
    evidence["open_blocking_threads"] = False

    with pytest.raises(ContractError, match="non-negative integer"):
        assert_join(head, evidence)


def test_join_evidence_validates_embedded_review_contract() -> None:
    head = "e" * 40
    evidence = join_evidence(head)
    evidence["general_review"]["actual_model"] = "openai-codex/other"

    with pytest.raises(ContractError, match="model substitution"):
        assert_join(head, evidence)


def test_join_evidence_requires_originating_reviewer_confirmation() -> None:
    head = "e" * 40
    finding_id = "GENERAL-R1-001"
    resolution = {
        "finding_id": finding_id,
        "resolution_commit": "d" * 40,
        "resolved_head_sha": head,
        "resolution_summary": "Fixed the invariant and added a regression test.",
        "tests": ["uv run pytest"],
    }
    evidence = join_evidence(head)
    evidence["builder_result"] = builder_result(head, resolutions=[resolution])
    evidence["mandatory_findings"] = [
        {"id": finding_id, "origin_role": "reviewer-general"}
    ]

    with pytest.raises(ContractError, match="lacks originating-reviewer confirmation"):
        assert_join(head, evidence)

    evidence["general_review"]["finding_confirmations"] = [
        {
            "finding_id": finding_id,
            "status": "CONFIRMED_RESOLVED",
            "reviewed_fix_sha": head,
            "evidence": ["Regression test passes"],
        }
    ]
    assert_join(head, evidence)


def test_join_evidence_rejects_non_ready_builder() -> None:
    head = "e" * 40
    evidence = join_evidence(head)
    evidence["builder_result"]["outcome"] = "BLOCKED"
    evidence["builder_result"]["github_ci_green"] = False

    with pytest.raises(ContractError, match="builder is not review-ready"):
        assert_join(head, evidence)


@pytest.mark.parametrize(
    ("record", "field", "value", "message"),
    [
        ("general_review", "case_id", "mdk#9999", "case mismatch"),
        ("builder_result", "pr_number", 9999, "PR mismatch"),
        ("secperf_review", "plan_version", 2, "plan-version mismatch"),
    ],
)
def test_join_evidence_rejects_spliced_context(
    record: str, field: str, value: object, message: str
) -> None:
    head = "e" * 40
    evidence = join_evidence(head)
    evidence[record][field] = value

    with pytest.raises(ContractError, match=message):
        assert_join(head, evidence)


def test_built_wheel_contains_contract_schemas(tmp_path: Path) -> None:
    subprocess.run(
        ["uv", "build", "--wheel", "--out-dir", str(tmp_path)],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    wheel = next(tmp_path.glob("*.whl"))
    with zipfile.ZipFile(wheel) as archive:
        names = set(archive.namelist())
        app = tmp_path / "app"
        archive.extractall(app)

    expected = {
        f"pip_agent/schemas/{name}.schema.json"
        for name in (
            "case",
            "common-result",
            "planner-result",
            "builder-result",
            "review-result",
            "final-result",
        )
    }
    assert expected <= names
    assert "pip_agent/resources/canaries/mdk-1240-plan-v1.json" not in names
    assert "pip_agent/resources/manifests/roles/builder-grok.json" in names
    assert "pip_agent/resources/skills/builder-grok/SKILL.md" in names
    unit = tmp_path / "pip-v2-control.service"
    subprocess.run(
        [
            sys.executable,
            "-P",
            "-m",
            "pip_agent.control_plane",
            "render-unit",
            "--caller-group",
            "jeff",
            "--output",
            str(unit),
        ],
        cwd=tmp_path,
        env={**os.environ, "PYTHONPATH": str(app)},
        check=True,
    )
    app.rename(tmp_path / "release")
    assert "Type=notify" in unit.read_text()


def test_control_plane_install_artifacts_are_hardened_and_packaged() -> None:
    project = tomllib.loads((ROOT / "pyproject.toml").read_text())
    assert (
        project["project"]["scripts"]["pip-v2-control"]
        == "pip_agent.control_plane:main"
    )

    unit = render_service_unit("jeff")
    for directive in (
        "User=pip-v2-control",
        "Group=pip-v2-control",
        "Type=notify",
        "NotifyAccess=main",
        "SupplementaryGroups=jeff",
        "StateDirectory=pip-v2",
        "RuntimeDirectory=pip-v2",
        "ProtectSystem=strict",
        "ProtectHome=true",
        "NoNewPrivileges=true",
        "PrivateTmp=true",
        "RestrictAddressFamilies=AF_UNIX",
        "Environment=PYTHONSAFEPATH=1",
        "UMask=0077",
    ):
        assert directive in unit
    assert "pip-v2-control serve --config /etc/pip-v2/control.json" in unit

    decision_unit = render_decision_service_unit("jeff")
    for directive in (
        "User=pip-v2-control",
        "Group=pip-v2-control",
        "SupplementaryGroups=jeff",
        "Type=oneshot",
        "StateDirectory=pip-v2",
        "ProtectHome=true",
        "NoNewPrivileges=true",
        "RestrictAddressFamilies=AF_INET AF_INET6",
        "LoadCredential=github.token:/etc/pip-v2/github.token",
        "pip-v2-control reconcile-once --config /etc/pip-v2/control.json",
        "--route-output /run/pip-v2/decision-route.json",
    ):
        assert directive in decision_unit
    timer_unit = render_decision_timer_unit()
    assert "OnActiveSec=2m" in timer_unit
    assert "OnBootSec=" not in timer_unit
    assert "OnUnitActiveSec=5m" in timer_unit
    assert "OnUnitActiveSec=2m" not in timer_unit
    assert "Persistent=true" in timer_unit
    assert "Unit=pip-v2-decision.service" in timer_unit

    route_unit = render_route_consumer_service("jeff", "jeff", Path("/home/jeff"))
    for directive in (
        "User=jeff",
        "Group=jeff",
        "LoadCredential=github.token:/etc/pip-v2/github.token",
        "Type=oneshot",
        "NoNewPrivileges=true",
        "ProtectSystem=strict",
        "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6",
        "pip-v2-route-consumer consume",
        "--route /run/pip-v2/decision-route.json",
        "--board pip-mdk",
        "ReadWritePaths=/home/jeff/.hermes",
    ):
        assert directive in route_unit
    route_timer = render_route_consumer_timer()
    assert "OnActiveSec=15s" in route_timer
    assert "OnBootSec=" not in route_timer
    assert "OnUnitActiveSec=15s" in route_timer
    assert "Unit=pip-v2-route-consumer.service" in route_timer

    installer = ROOT / "scripts/install-control-plane.sh"
    subprocess.run(["bash", "-n", str(installer)], check=True)
    script = installer.read_text()
    assert "pip-v2-control" in script
    assert "useradd --system" in script
    assert "export PYTHONDONTWRITEBYTECODE=1 PYTHONSAFEPATH=1" in script
    assert "groupadd --system" in script
    assert "usermod" not in script
    assert "pip-v2-control identity is not exclusively configured" in script
    assert "userdel pip-v2-control" in script
    assert "groupdel pip-v2-control" in script
    assert 'rm -rf -- "$RELEASE_DIR"' in script
    assert "systemctl enable pip-v2-control.service" in script
    assert "systemctl enable pip-v2-decision.timer" in script
    assert "systemctl start pip-v2-decision.service" not in script
    assert "decision_reconciliation_pending" in script
    assert "GITHUB_CREDENTIAL=/etc/pip-v2/github.token" in script
    assert "GitHub credential must be a root-owned mode-0600 regular file" in script
    assert "HAD_DECISION_ROUTE" in script
    assert "DECISION_ROUTE_REPLACED" in script
    assert "failed to quiesce $unit" in script
    assert script.index("existing decision route is unsafe") < script.index(
        "MUTATION_STARTED=1"
    )
    assert script.index("systemctl stop pip-v2-route-consumer.timer") < script.index(
        "mv -Tf /opt/pip-v2/current.new /opt/pip-v2/current"
    )
    assert script.index("control-plane boundary validation failed") < script.rindex(
        "systemctl start pip-v2-decision.timer"
    )
    assert "systemctl enable pip-v2-route-consumer.timer" in script
    assert "/etc/systemd/system/pip-v2-decision.service" in script
    assert "/etc/systemd/system/pip-v2-decision.timer" in script
    assert "/etc/systemd/system/pip-v2-route-consumer.service" in script
    assert "/etc/systemd/system/pip-v2-route-consumer.timer" in script
    assert "systemctl restart pip-v2-control.service" in script
    assert "--installer-sha256" in script
    assert "installer SHA-256 mismatch" in script
    assert "installer must be a root-owned" in script
    assert "PATH=/usr/sbin:/usr/bin:/sbin:/bin" in script
    assert "/usr/bin/python3 -P" in script
    assert script.index("render-unit") < script.index('mv "$INSTALL_TMP/app"')
    assert "--sha256" in script
    assert "wheel SHA-256 mismatch" in script
    assert '[[ "$ISSUE" == "1240" ]]' in script
    assert 'runuser -u "$CALLER" -- test -e /var/lib/pip-v2/cases.db' in script
    assert "control-plane boundary validation failed" in script
    assert "$CALLER_HOME/code" not in script
    assert "/var/lib/pip-v2-router" not in script
    assert 'runuser -u "$CALLER" -- /usr/bin/env' in script
    assert "-m pip_agent.bootstrap" in script
    assert "CREATED_CURSOR_LINKS" in script
    assert "PROFILE_SNAPSHOT_READY" in script
    assert '"$INSTALL_TMP/profile-snapshot/state.json"' in script
    assert "refusing to snapshot unsafe profile root" in script
    assert "shutil.rmtree(profile)" in script
    assert "--apply" in script
    assert "os.readlink(path) == expected" in script
    assert "rollback_install" in script
    assert "restoring the previous control-plane deployment" in script
    assert "chmod 0640" in script
