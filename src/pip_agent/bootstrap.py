from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
from collections.abc import Callable, Mapping, Sequence
from dataclasses import asdict, dataclass
from pathlib import Path

from .manifests import RoleManifest, load_role_manifests


class BootstrapError(RuntimeError):
    """Profile bootstrap cannot proceed without overwriting foreign state."""


@dataclass(frozen=True)
class ProfileAction:
    profile: str
    provider: str
    model: str
    reasoning: str
    toolsets: tuple[str, ...]
    description: str


PROFILE_MARKER = "pip-v2-profile.json"

CLI_TOOLSET_CATALOG = (
    "browser",
    "clarify",
    "code_execution",
    "computer_use",
    "cronjob",
    "delegation",
    "file",
    "image_gen",
    "video",
    "video_gen",
    "x_search",
    "kanban",
    "memory",
    "session_search",
    "skills",
    "terminal",
    "todo",
    "tts",
    "vision",
    "web",
    "context_engine",
    "homeassistant",
    "spotify",
    "yuanbao",
    "platform",
)


def plan_profiles(roles: Mapping[str, RoleManifest]) -> list[ProfileAction]:
    return [
        ProfileAction(
            profile=role.name,
            provider=role.provider,
            model=role.model,
            reasoning=role.reasoning,
            toolsets=role.toolsets,
            description=role.description,
        )
        for role in sorted(roles.values(), key=lambda item: item.name)
        if role.execution == "hermes"
    ]


def install_skill_links(
    *, profile_home: Path, repo_root: Path, role_name: str
) -> list[Path]:
    skills_home = profile_home / "skills"
    skills_home.mkdir(parents=True, exist_ok=True)
    targets = {
        role_name: (repo_root / "skills" / role_name).resolve(),
        "workflow-contract": (
            repo_root / "skills" / "shared" / "workflow-contract"
        ).resolve(),
    }
    installed: list[Path] = []
    for link_name, target in sorted(targets.items()):
        if not target.is_dir():
            raise BootstrapError(f"canonical skill is missing: {target}")
        link = skills_home / link_name
        if link.is_symlink():
            if link.resolve(strict=False) != target:
                raise BootstrapError(f"refusing to replace foreign symlink: {link}")
        elif link.exists():
            raise BootstrapError(f"refusing to replace existing skill path: {link}")
        else:
            link.symlink_to(target, target_is_directory=True)
        installed.append(link)
    return installed


def install_shared_auth_links(profile_home: Path, shared_home: Path) -> list[Path]:
    """Link OAuth state and its lock so profiles cannot race copied token pools."""
    links: list[Path] = []
    for name in ("auth.json", "auth.lock"):
        target = (shared_home / name).resolve()
        if not target.is_file():
            raise BootstrapError(f"shared authentication path is missing: {target}")
        link = profile_home / name
        if link.is_symlink():
            if link.resolve(strict=False) != target:
                raise BootstrapError(
                    f"refusing to replace foreign auth symlink: {link}"
                )
        elif link.exists():
            raise BootstrapError(f"refusing to replace existing auth path: {link}")
        else:
            link.symlink_to(target)
        links.append(link)
    return links


def _expected_links_exist(
    profile_home: Path, repo_root: Path, hermes_home: Path, role_name: str
) -> bool:
    expected = {
        profile_home / "skills" / role_name: (
            repo_root / "skills" / role_name
        ).resolve(),
        profile_home / "skills" / "workflow-contract": (
            repo_root / "skills/shared/workflow-contract"
        ).resolve(),
        profile_home / "auth.json": (hermes_home / "auth.json").resolve(),
        profile_home / "auth.lock": (hermes_home / "auth.lock").resolve(),
    }
    return all(
        link.is_symlink() and link.resolve(strict=False) == target
        for link, target in expected.items()
    )


def _ensure_profile_marker(
    *,
    profile_home: Path,
    repo_root: Path,
    hermes_home: Path,
    role_name: str,
    created: bool,
) -> None:
    marker = profile_home / PROFILE_MARKER
    expected = {
        "managed_by": "agent-p1p",
        "workflow_version": 2,
        "role": role_name,
        "repo_root": str(repo_root.resolve()),
    }
    if marker.exists():
        try:
            current = json.loads(marker.read_text())
        except (OSError, json.JSONDecodeError) as exc:
            raise BootstrapError(f"invalid profile marker: {marker}") from exc
        if current != expected:
            raise BootstrapError(f"profile marker does not match manifest: {marker}")
        return
    if not created and not _expected_links_exist(
        profile_home, repo_root, hermes_home, role_name
    ):
        raise BootstrapError(
            f"refusing to adopt foreign Hermes profile: {profile_home}"
        )
    marker.write_text(json.dumps(expected, indent=2, sort_keys=True) + "\n")


def _run(command: Sequence[str], env: Mapping[str, str] | None = None) -> None:
    completed = subprocess.run(
        command,
        check=False,
        text=True,
        capture_output=True,
        env=dict(env) if env is not None else None,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise BootstrapError(f"Hermes command failed: {detail}")


def _discover_toolsets(env: Mapping[str, str]) -> tuple[str, ...]:
    completed = subprocess.run(
        ["hermes", "tools", "list", "--platform", "cli"],
        check=True,
        text=True,
        capture_output=True,
        env=dict(env),
    )
    names = {
        match.group(1)
        for line in completed.stdout.splitlines()
        if (match := re.match(r"^\s*[✓✗]\s+(?:enabled|disabled)\s+(\S+)", line))
    }
    if not names:
        raise BootstrapError("Hermes returned no discoverable CLI toolsets")
    return tuple(sorted(names | {"kanban"}))


def _verify_toolsets(
    profile: str, expected: Sequence[str], env: Mapping[str, str]
) -> None:
    completed = subprocess.run(
        [
            "hermes",
            "-p",
            profile,
            "config",
            "get",
            "platform_toolsets.cli",
            "--json",
        ],
        check=True,
        text=True,
        capture_output=True,
        env=dict(env),
    )
    try:
        actual = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise BootstrapError(f"cannot verify toolsets for {profile}") from exc
    if not isinstance(actual, list) or set(actual) != set(expected):
        raise BootstrapError(
            f"effective toolsets for {profile} differ: {actual!r} != {list(expected)!r}"
        )


def apply_profiles(
    *,
    repo_root: Path,
    hermes_home: Path,
    runner: Callable[[Sequence[str], Mapping[str, str] | None], None] | None = None,
) -> list[dict[str, object]]:
    roles = load_role_manifests(repo_root / "manifests" / "roles")
    environment = dict(os.environ)
    environment["HERMES_HOME"] = str(hermes_home.resolve())
    effective_runner = runner or _run
    toolset_catalog = (
        _discover_toolsets(environment) if runner is None else CLI_TOOLSET_CATALOG
    )
    reports: list[dict[str, object]] = []
    for action in plan_profiles(roles):
        profile_home = hermes_home / "profiles" / action.profile
        created = False
        if not profile_home.exists():
            effective_runner(
                [
                    "hermes",
                    "profile",
                    "create",
                    action.profile,
                    "--no-skills",
                    "--description",
                    action.description,
                ],
                environment,
            )
            created = True
        if not profile_home.is_dir():
            raise BootstrapError(f"Hermes profile was not created: {profile_home}")
        _ensure_profile_marker(
            profile_home=profile_home,
            repo_root=repo_root,
            hermes_home=hermes_home,
            role_name=action.profile,
            created=created,
        )
        links = install_skill_links(
            profile_home=profile_home,
            repo_root=repo_root,
            role_name=action.profile,
        )
        auth_links = install_shared_auth_links(profile_home, hermes_home)
        for key, value in (
            ("model.provider", action.provider),
            ("model.default", action.model),
        ):
            effective_runner(
                ["hermes", "-p", action.profile, "config", "set", key, value],
                environment,
            )
        effective_runner(
            [
                "hermes",
                "-p",
                action.profile,
                "config",
                "set",
                "model.reasoning_effort",
                action.reasoning,
            ],
            environment,
        )
        effective_runner(
            [
                "hermes",
                "-p",
                action.profile,
                "config",
                "set",
                "kanban.max_in_progress_per_profile",
                "1",
            ],
            environment,
        )
        effective_runner(
            ["hermes", "-p", action.profile, "fallback", "clear"], environment
        )
        for toolset in toolset_catalog:
            effective_runner(
                [
                    "hermes",
                    "-p",
                    action.profile,
                    "tools",
                    "disable",
                    toolset,
                    "--platform",
                    "cli",
                ],
                environment,
            )
        for toolset in action.toolsets:
            effective_runner(
                [
                    "hermes",
                    "-p",
                    action.profile,
                    "tools",
                    "enable",
                    toolset,
                    "--platform",
                    "cli",
                ],
                environment,
            )
        if runner is None:
            _verify_toolsets(action.profile, action.toolsets, environment)
        reports.append(
            {
                **asdict(action),
                "profile_home": str(profile_home),
                "skill_links": [str(link) for link in links],
                "shared_auth_links": [str(link) for link in auth_links],
            }
        )
    return reports


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Bootstrap Pip v2 Hermes profiles")
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--hermes-home", type=Path, default=Path.home() / ".hermes")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args(argv)
    roles = load_role_manifests(args.repo_root / "manifests" / "roles")
    if args.apply:
        result = apply_profiles(
            repo_root=args.repo_root.resolve(), hermes_home=args.hermes_home.resolve()
        )
    else:
        result = [asdict(action) for action in plan_profiles(roles)]
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
