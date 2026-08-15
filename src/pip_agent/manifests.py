from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Literal


class ManifestError(ValueError):
    """A role manifest is missing, malformed, or contradictory."""


@dataclass(frozen=True)
class RoleManifest:
    name: str
    execution: Literal["hermes", "cursor"]
    provider: str
    model: str
    reasoning: Literal["high", "xhigh"]
    outer_reasoning_model: None
    fresh_session: bool
    skills: tuple[str, ...]
    toolsets: tuple[str, ...]
    result_schema: str
    description: str

    @property
    def contract_model(self) -> str:
        return f"{self.provider}/{self.model}"


_REQUIRED_KEYS = frozenset(RoleManifest.__dataclass_fields__)


def load_role_manifests(directory: Path) -> dict[str, RoleManifest]:
    if not directory.is_dir():
        raise ManifestError(f"role manifest directory does not exist: {directory}")
    roles: dict[str, RoleManifest] = {}
    for path in sorted(directory.glob("*.json")):
        try:
            raw = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            raise ManifestError(f"cannot load {path}: {exc}") from exc
        if not isinstance(raw, dict) or set(raw) != _REQUIRED_KEYS:
            raise ManifestError(
                f"{path.name} must contain exactly the role manifest keys"
            )
        try:
            role = RoleManifest(
                name=raw["name"],
                execution=raw["execution"],
                provider=raw["provider"],
                model=raw["model"],
                reasoning=raw["reasoning"],
                outer_reasoning_model=raw["outer_reasoning_model"],
                fresh_session=raw["fresh_session"],
                skills=tuple(raw["skills"]),
                toolsets=tuple(raw["toolsets"]),
                result_schema=raw["result_schema"],
                description=raw["description"],
            )
        except (KeyError, TypeError) as exc:
            raise ManifestError(f"invalid role manifest {path.name}: {exc}") from exc
        _validate_role(role, path)
        if role.name in roles:
            raise ManifestError(f"duplicate role {role.name}")
        roles[role.name] = role
    if not roles:
        raise ManifestError("no role manifests found")
    return roles


def _validate_role(role: RoleManifest, path: Path) -> None:
    if path.stem != role.name:
        raise ManifestError(f"{path.name} does not match role name {role.name!r}")
    if role.execution not in {"hermes", "cursor"}:
        raise ManifestError(f"unsupported execution type for {role.name}")
    if role.reasoning not in {"high", "xhigh"}:
        raise ManifestError(f"unsupported reasoning level for {role.name}")
    if role.outer_reasoning_model is not None:
        raise ManifestError(f"nested reasoning is forbidden for {role.name}")
    if role.fresh_session is not True:
        raise ManifestError(f"fresh sessions are mandatory for {role.name}")
    if role.skills != ("workflow-contract", role.name):
        raise ManifestError(f"{role.name} must load shared policy before role policy")
    if role.execution == "cursor" and role.provider != "cursor":
        raise ManifestError(f"direct Cursor role {role.name} has wrong provider")
    if role.execution == "cursor" and role.toolsets:
        raise ManifestError(
            f"direct Cursor role {role.name} cannot declare Hermes tools"
        )
    if role.execution == "hermes" and role.provider != "openai-codex":
        raise ManifestError(f"Hermes role {role.name} has wrong provider")
    for value in (
        role.name,
        role.provider,
        role.model,
        role.result_schema,
        role.description,
    ):
        if not isinstance(value, str) or not value:
            raise ManifestError(f"{role.name} has an empty or non-string field")
