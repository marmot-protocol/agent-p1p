from __future__ import annotations

import json
import re
from collections.abc import Mapping
from datetime import datetime
from importlib import resources
from pathlib import Path
from typing import Any

try:
    from jsonschema import Draft202012Validator, FormatChecker
    from jsonschema.exceptions import SchemaError, ValidationError
except ModuleNotFoundError:  # Runtime releases are dependency-free wheel extracts.
    Draft202012Validator = None  # type: ignore[assignment,misc]
    FormatChecker = None  # type: ignore[assignment,misc]
    SchemaError = ValueError  # type: ignore[assignment,misc]
    ValidationError = ValueError  # type: ignore[assignment,misc]

SOURCE_SCHEMA_DIR = Path(__file__).resolve().parents[2] / "schemas"
SCHEMA_REGISTRY = frozenset(
    {
        "case",
        "common-result",
        "planner-result",
        "builder-result",
        "review-result",
        "final-result",
    }
)
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")


class ContractError(ValueError):
    """A worker result or deterministic evidence bundle violated its contract."""


def _schema_text(name: str) -> str:
    if name not in SCHEMA_REGISTRY:
        raise ContractError(f"unknown schema {name!r}")

    source_path = SOURCE_SCHEMA_DIR / f"{name}.schema.json"
    if source_path.is_file():
        return source_path.read_text()

    try:
        return (
            resources.files("pip_agent")
            .joinpath("schemas", f"{name}.schema.json")
            .read_text()
        )
    except (FileNotFoundError, ModuleNotFoundError, OSError) as exc:
        raise ContractError(f"schema {name!r} is not installed") from exc


def load_schema(name: str) -> dict[str, Any]:
    try:
        schema = json.loads(_schema_text(name))
        if Draft202012Validator is not None:
            Draft202012Validator.check_schema(schema)
    except (json.JSONDecodeError, SchemaError) as exc:
        raise ContractError(f"invalid schema {name}: {exc}") from exc
    return schema


def _minimal_validate(
    schema: Mapping[str, Any], value: Any, path: str = "<root>"
) -> None:
    expected_type = schema.get("type")
    valid_type = {
        "object": lambda item: isinstance(item, dict),
        "array": lambda item: isinstance(item, list),
        "string": lambda item: isinstance(item, str),
        "integer": lambda item: type(item) is int,
        "boolean": lambda item: type(item) is bool,
    }.get(expected_type)
    if valid_type is not None and not valid_type(value):
        raise ContractError(f"contract violation at {path}: expected {expected_type}")
    if "const" in schema and value != schema["const"]:
        raise ContractError(f"contract violation at {path}: unexpected value")
    if "enum" in schema and value not in schema["enum"]:
        raise ContractError(f"contract violation at {path}: value is not allowed")
    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            raise ContractError(f"contract violation at {path}: string is too short")
        pattern = schema.get("pattern")
        if isinstance(pattern, str) and re.fullmatch(pattern, value) is None:
            raise ContractError(f"contract violation at {path}: pattern mismatch")
        if schema.get("format") == "date-time":
            try:
                datetime.fromisoformat(value)
            except ValueError as exc:
                raise ContractError(
                    f"contract violation at {path}: invalid date-time"
                ) from exc
    if type(value) is int and value < schema.get("minimum", value):
        raise ContractError(f"contract violation at {path}: value is too small")
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            raise ContractError(f"contract violation at {path}: too few items")
        item_schema = schema.get("items")
        if isinstance(item_schema, dict):
            for index, item in enumerate(value):
                _minimal_validate(item_schema, item, f"{path}.{index}")
    if isinstance(value, dict):
        properties = schema.get("properties", {})
        for required in schema.get("required", []):
            if required not in value:
                raise ContractError(
                    f"contract violation at {path}: missing {required!r}"
                )
        if schema.get("additionalProperties") is False:
            extra = set(value) - set(properties)
            if extra:
                raise ContractError(
                    f"contract violation at {path}: unexpected {sorted(extra)!r}"
                )
        for key, item in value.items():
            item_schema = properties.get(key)
            if isinstance(item_schema, dict):
                _minimal_validate(item_schema, item, f"{path}.{key}")
    for condition in schema.get("allOf", []):
        conditional = condition.get("if")
        applies = True
        if isinstance(conditional, dict):
            try:
                _minimal_validate(conditional, value, path)
            except ContractError:
                applies = False
        selected = condition.get("then" if applies else "else")
        if isinstance(selected, dict):
            _minimal_validate(selected, value, path)


def validate_contract(name: str, payload: Mapping[str, Any]) -> None:
    schema = load_schema(name)
    if Draft202012Validator is None:
        _minimal_validate(schema, dict(payload))
    else:
        try:
            Draft202012Validator(schema, format_checker=FormatChecker()).validate(
                dict(payload)
            )
        except ValidationError as exc:
            location = ".".join(str(part) for part in exc.absolute_path) or "<root>"
            raise ContractError(
                f"{name} contract violation at {location}: {exc.message}"
            ) from exc

    requested = payload.get("requested_model")
    actual = payload.get("actual_model")
    outcome = payload.get("outcome")
    if (
        requested is not None
        and requested != actual
        and outcome != "BLOCKED_UNEXPECTED_MODEL"
    ):
        raise ContractError(
            f"model substitution is forbidden: requested {requested!r}, actual {actual!r}"
        )
    if requested == actual and outcome == "BLOCKED_UNEXPECTED_MODEL":
        raise ContractError(
            "BLOCKED_UNEXPECTED_MODEL requires an actual model mismatch"
        )

    if name == "review-result":
        if payload.get("outcome") == "APPROVE" and payload.get("blocking_findings"):
            raise ContractError("APPROVE review cannot contain blocking findings")
        if payload.get("outcome") == "APPROVE" and any(
            item.get("status") != "CONFIRMED_RESOLVED"
            for item in payload.get("finding_confirmations", [])
        ):
            raise ContractError("APPROVE review cannot retain an open finding")

    if name == "builder-result":
        if payload.get("outcome") == "REVIEW_READY":
            if payload.get("github_ci_green") is not True:
                raise ContractError("REVIEW_READY requires green CI")
            if payload.get("head_sha") != payload.get("ci_head_sha"):
                raise ContractError("CI head does not match the current PR head")
        elif payload.get("github_ci_green") is True:
            raise ContractError("green CI handoff must use REVIEW_READY outcome")


def assert_exact_head_evidence(
    case_id: str,
    pr_number: int,
    plan_version: int,
    head_sha: str,
    evidence: Mapping[str, Any],
) -> None:
    if not case_id:
        raise ContractError("trusted case ID is required")
    if type(pr_number) is not int or pr_number < 1:
        raise ContractError("trusted PR number must be a positive integer")
    if type(plan_version) is not int or plan_version < 1:
        raise ContractError("trusted plan version must be a positive integer")
    if not SHA_PATTERN.fullmatch(head_sha):
        raise ContractError("head SHA must be a valid 40-character lowercase SHA")

    checks = (
        ("reviewer-general", evidence.get("general_review")),
        ("reviewer-secperf", evidence.get("secperf_review")),
    )
    for role, review in checks:
        if not isinstance(review, Mapping):
            raise ContractError(f"{role} review evidence is missing")
        validate_contract("review-result", review)
        if review.get("role") != role:
            raise ContractError(f"{role} evidence has the wrong role")
        if review.get("case_id") != case_id:
            raise ContractError(f"{role} case mismatch")
        if review.get("pr_number") != pr_number:
            raise ContractError(f"{role} PR mismatch")
        if review.get("plan_version") != plan_version:
            raise ContractError(f"{role} plan-version mismatch")
        if review.get("outcome") != "APPROVE":
            raise ContractError(f"{role} has not approved")
        if review.get("reviewed_head_sha") != head_sha:
            raise ContractError(f"{role} reviewed stale head")

    builder = evidence.get("builder_result")
    if not isinstance(builder, Mapping):
        raise ContractError("builder result evidence is missing")
    validate_contract("builder-result", builder)
    if builder.get("outcome") != "REVIEW_READY":
        raise ContractError("builder is not review-ready")
    if builder.get("case_id") != case_id:
        raise ContractError("builder case mismatch")
    if builder.get("pr_number") != pr_number:
        raise ContractError("builder PR mismatch")
    if builder.get("plan_version") != plan_version:
        raise ContractError("builder plan-version mismatch")
    if builder.get("head_sha") != head_sha:
        raise ContractError("builder result is stale")

    mandatory_findings = evidence.get("mandatory_findings")
    if not isinstance(mandatory_findings, list):
        raise ContractError("mandatory_findings must be a list")
    resolutions = _index_unique(
        builder.get("finding_resolutions", []), "finding_id", "builder resolution"
    )
    reviews = {
        "reviewer-general": evidence["general_review"],
        "reviewer-secperf": evidence["secperf_review"],
    }
    seen_findings: set[str] = set()
    for finding in mandatory_findings:
        if not isinstance(finding, Mapping):
            raise ContractError("mandatory finding must be an object")
        finding_id = finding.get("id")
        origin_role = finding.get("origin_role")
        if not isinstance(finding_id, str) or not re.fullmatch(
            r"[A-Z]+-R[0-9]+-[0-9]{3}", finding_id
        ):
            raise ContractError("mandatory finding has an invalid ID")
        if finding_id in seen_findings:
            raise ContractError(f"duplicate mandatory finding {finding_id}")
        seen_findings.add(finding_id)
        if origin_role not in reviews:
            raise ContractError(f"mandatory finding {finding_id} has invalid origin")
        resolution = resolutions.get(finding_id)
        if resolution is None or resolution.get("resolved_head_sha") != head_sha:
            raise ContractError(
                f"mandatory finding {finding_id} lacks exact-head resolution"
            )
        confirmations = _index_unique(
            reviews[origin_role].get("finding_confirmations", []),
            "finding_id",
            f"{origin_role} confirmation",
        )
        confirmation = confirmations.get(finding_id)
        if (
            confirmation is None
            or confirmation.get("status") != "CONFIRMED_RESOLVED"
            or confirmation.get("reviewed_fix_sha") != head_sha
        ):
            raise ContractError(
                f"mandatory finding {finding_id} lacks originating-reviewer confirmation"
            )

    ci = evidence.get("ci")
    if not isinstance(ci, Mapping) or ci.get("head_sha") != head_sha:
        raise ContractError("CI evidence is stale or missing")
    if ci.get("completed") is not True:
        raise ContractError("CI evidence is incomplete")
    if ci.get("hollow") is not False or ci.get("rate_limited") is not False:
        raise ContractError("CI evidence is hollow or rate-limited")
    if ci.get("required_checks_green") is not True:
        raise ContractError("required CI is not green")
    value = evidence.get("open_blocking_threads")
    if type(value) is not int or value < 0:
        raise ContractError("open_blocking_threads must be a non-negative integer")
    if evidence["open_blocking_threads"] != 0:
        raise ContractError("blocking review threads remain")
    if evidence.get("pip_owned") is not True:
        raise ContractError("PR and branch are not confirmed Pip-owned")
    if evidence.get("mergeable") is not True:
        raise ContractError("PR is not cleanly mergeable")
    if evidence.get("issue_authorization_valid") is not True:
        raise ContractError("issue authorization is no longer valid")


def _index_unique(items: Any, key: str, label: str) -> dict[str, Mapping[str, Any]]:
    if not isinstance(items, list):
        raise ContractError(f"{label}s must be a list")
    indexed: dict[str, Mapping[str, Any]] = {}
    for item in items:
        if not isinstance(item, Mapping) or not isinstance(item.get(key), str):
            raise ContractError(f"{label} must contain a string {key}")
        value = item[key]
        if value in indexed:
            raise ContractError(f"duplicate {label} for {value}")
        indexed[value] = item
    return indexed
