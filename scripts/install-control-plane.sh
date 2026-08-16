#!/bin/bash
set -Eeuo pipefail

PATH=/usr/sbin:/usr/bin:/sbin:/bin
export PATH
unset PYTHONPATH PYTHONHOME PYTHONSTARTUP
umask 077

usage() {
  echo "usage: sudo $0 --installer-sha256 HEX --wheel /absolute/path.whl --sha256 HEX --caller USER --issue 1240" >&2
  exit 2
}

WHEEL=""
EXPECTED_SHA=""
EXPECTED_INSTALLER_SHA=""
CALLER="${SUDO_USER:-}"
ISSUE=""
while (($#)); do
  case "$1" in
    --installer-sha256) [[ $# -ge 2 ]] || usage; EXPECTED_INSTALLER_SHA="${2,,}"; shift 2 ;;
    --wheel) [[ $# -ge 2 ]] || usage; WHEEL="$2"; shift 2 ;;
    --sha256) [[ $# -ge 2 ]] || usage; EXPECTED_SHA="${2,,}"; shift 2 ;;
    --caller) [[ $# -ge 2 ]] || usage; CALLER="$2"; shift 2 ;;
    --issue) [[ $# -ge 2 ]] || usage; ISSUE="$2"; shift 2 ;;
    *) usage ;;
  esac
done

[[ $EUID -eq 0 ]] || { echo "must run as root" >&2; exit 1; }
[[ -n "$WHEEL" && -n "$CALLER" && "$ISSUE" =~ ^[1-9][0-9]*$ ]] || usage
[[ "$EXPECTED_SHA" =~ ^[0-9a-f]{64}$ ]] || usage
[[ "$EXPECTED_INSTALLER_SHA" =~ ^[0-9a-f]{64}$ ]] || usage
SCRIPT_UID="$(stat -c %u "${BASH_SOURCE[0]}")"
SCRIPT_MODE="$(stat -c %a "${BASH_SOURCE[0]}")"
[[ "$SCRIPT_UID" == "0" && $((8#$SCRIPT_MODE & 8#22)) -eq 0 ]] || {
  echo "installer must be a root-owned, non-group/world-writable pinned copy" >&2
  exit 1
}
ACTUAL_INSTALLER_SHA="$(sha256sum "${BASH_SOURCE[0]}" | awk '{print $1}')"
[[ "$ACTUAL_INSTALLER_SHA" == "$EXPECTED_INSTALLER_SHA" ]] || {
  echo "installer SHA-256 mismatch" >&2
  exit 1
}
[[ "$ISSUE" == "1240" ]] || {
  echo "this installer is restricted to marmot-protocol/mdk#1240" >&2
  exit 1
}
WHEEL="$(realpath --canonicalize-existing "$WHEEL")"
[[ -f "$WHEEL" && ! -L "$WHEEL" && "$WHEEL" == *.whl ]] || {
  echo "wheel must be a regular, non-symlink .whl file" >&2
  exit 1
}
cd /

INSTALL_TMP="$(mktemp -d /var/tmp/pip-v2.install.XXXXXX)"
MUTATION_STARTED=0
OLD_TARGET=""
OLD_ENABLED=0
OLD_ACTIVE=0
OLD_TIMER_ENABLED=0
OLD_TIMER_ACTIVE=0
OLD_ROUTER_TIMER_ENABLED=0
OLD_ROUTER_TIMER_ACTIVE=0
HAD_WRAPPER=0
HAD_ROUTER_WRAPPER=0
HAD_CONFIG=0
HAD_UNIT=0
HAD_DECISION_UNIT=0
HAD_DECISION_TIMER=0
HAD_ROUTER_UNIT=0
HAD_ROUTER_TIMER=0
HAD_WHEEL_SHA=0
CREATED_GROUP=0
CREATED_USER=0
CREATED_RELEASE=0
CREATED_OPT_DIR=0
CREATED_RELEASES_DIR=0
CREATED_ETC_DIR=0
CREATED_BOARD=0
RELEASE_DIR=""
CALLER_HOME=""
CREATED_CURSOR_LINKS=()
PROFILE_SNAPSHOT_READY=0

restore_file() {
  local had="$1" backup="$2" destination="$3"
  if [[ "$had" == "1" ]]; then
    cp -a -- "$backup" "$destination"
  else
    rm -f -- "$destination"
  fi
}

rollback_install() {
  set +e
  systemctl stop pip-v2-route-consumer.timer pip-v2-route-consumer.service >/dev/null 2>&1
  systemctl stop pip-v2-decision.timer pip-v2-decision.service >/dev/null 2>&1
  systemctl stop pip-v2-control.service >/dev/null 2>&1
  if [[ -n "$OLD_TARGET" ]]; then
    ln -sfn "$OLD_TARGET" /opt/pip-v2/current.rollback
    mv -Tf /opt/pip-v2/current.rollback /opt/pip-v2/current
  else
    rm -f /opt/pip-v2/current
  fi
  for link in "${CREATED_CURSOR_LINKS[@]}"; do
    runuser -u "$CALLER" -- /usr/bin/python3 -P - "$link" "$CALLER_HOME" <<'PY'
import os
import sys
from pathlib import Path

path = Path(sys.argv[1])
home = Path(sys.argv[2])
expected = {
    home / ".hermes/profiles/cursor-fixer/skills/builder-grok": "/opt/pip-v2/current/pip_agent/resources/skills/builder-grok",
    home / ".hermes/profiles/cursor-fixer/skills/workflow-contract": "/opt/pip-v2/current/pip_agent/resources/skills/shared/workflow-contract",
    home / ".hermes/profiles/cursor-reviewer/skills/reviewer-secperf": "/opt/pip-v2/current/pip_agent/resources/skills/reviewer-secperf",
    home / ".hermes/profiles/cursor-reviewer/skills/workflow-contract": "/opt/pip-v2/current/pip_agent/resources/skills/shared/workflow-contract",
}.get(path)
if expected is not None and path.is_symlink() and os.readlink(path) == expected:
    path.unlink()
PY
  done
  if [[ "$PROFILE_SNAPSHOT_READY" == "1" ]]; then
    runuser -u "$CALLER" -- /usr/bin/python3 -P - \
      "$INSTALL_TMP/profile-snapshot/state.json" "$CALLER_HOME" <<'PY'
import base64
import json
import os
import shutil
import sys
from pathlib import Path

state = json.loads(Path(sys.argv[1]).read_text())
profiles = Path(sys.argv[2]) / ".hermes/profiles"
for role, record in state.items():
    profile = profiles / role
    if not record["existed"]:
        if profile.is_symlink():
            raise SystemExit(f"refusing to remove symlinked profile root: {profile}")
        if profile.exists():
            shutil.rmtree(profile)
        continue
    if profile.is_symlink() or (profile.exists() and not profile.is_dir()):
        raise SystemExit(f"refusing to restore unsafe profile root: {profile}")
    profile.mkdir(parents=True, exist_ok=True)
    skills = profile / "skills"
    if skills.is_symlink() or (skills.exists() and not skills.is_dir()):
        raise SystemExit(f"refusing to restore unsafe skills directory: {skills}")
    for relative, entry in record["entries"].items():
        path = profile / relative
        if path.is_symlink() or path.is_file():
            path.unlink()
        elif path.exists():
            raise SystemExit(f"refusing to replace unexpected profile path: {path}")
        if entry is None:
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        if entry["type"] == "symlink":
            path.symlink_to(entry["target"])
        else:
            path.write_bytes(base64.b64decode(entry["data"]))
            os.chmod(path, entry["mode"])
    if record["skills"]["existed"]:
        skills.mkdir(exist_ok=True)
        os.chmod(skills, record["skills"]["mode"])
    elif skills.exists():
        shutil.rmtree(skills)
    os.chmod(profile, record["mode"])
PY
  fi
  if [[ "$CREATED_BOARD" == "1" ]]; then
    runuser -u "$CALLER" -- /usr/bin/env \
      HOME="$CALLER_HOME" \
      PATH="$CALLER_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin" \
      hermes kanban boards rm pip-mdk --delete >/dev/null 2>&1
  fi
  restore_file "$HAD_WRAPPER" "$INSTALL_TMP/backup/wrapper" /usr/local/bin/pip-v2-control
  restore_file "$HAD_ROUTER_WRAPPER" "$INSTALL_TMP/backup/router-wrapper" /usr/local/bin/pip-v2-route-consumer
  restore_file "$HAD_CONFIG" "$INSTALL_TMP/backup/config" /etc/pip-v2/control.json
  restore_file "$HAD_UNIT" "$INSTALL_TMP/backup/unit" /etc/systemd/system/pip-v2-control.service
  restore_file "$HAD_DECISION_UNIT" "$INSTALL_TMP/backup/decision-unit" /etc/systemd/system/pip-v2-decision.service
  restore_file "$HAD_DECISION_TIMER" "$INSTALL_TMP/backup/decision-timer" /etc/systemd/system/pip-v2-decision.timer
  restore_file "$HAD_ROUTER_UNIT" "$INSTALL_TMP/backup/router-unit" /etc/systemd/system/pip-v2-route-consumer.service
  restore_file "$HAD_ROUTER_TIMER" "$INSTALL_TMP/backup/router-timer" /etc/systemd/system/pip-v2-route-consumer.timer
  restore_file "$HAD_WHEEL_SHA" "$INSTALL_TMP/backup/wheel-sha" /opt/pip-v2/WHEEL.SHA256
  systemctl daemon-reload >/dev/null 2>&1
  if [[ "$OLD_ENABLED" == "1" ]]; then
    systemctl enable pip-v2-control.service >/dev/null 2>&1
  else
    systemctl disable pip-v2-control.service >/dev/null 2>&1
  fi
  if [[ "$OLD_TIMER_ENABLED" == "1" ]]; then
    systemctl enable pip-v2-decision.timer >/dev/null 2>&1
  else
    systemctl disable pip-v2-decision.timer >/dev/null 2>&1
  fi
  if [[ "$OLD_ROUTER_TIMER_ENABLED" == "1" ]]; then
    systemctl enable pip-v2-route-consumer.timer >/dev/null 2>&1
  else
    systemctl disable pip-v2-route-consumer.timer >/dev/null 2>&1
  fi
  if [[ "$OLD_ACTIVE" == "1" ]]; then
    systemctl start pip-v2-control.service >/dev/null 2>&1
  fi
  if [[ "$OLD_TIMER_ACTIVE" == "1" ]]; then
    systemctl start pip-v2-decision.timer >/dev/null 2>&1
  fi
  if [[ "$OLD_ROUTER_TIMER_ACTIVE" == "1" ]]; then
    systemctl start pip-v2-route-consumer.timer >/dev/null 2>&1
  fi
  set -e
}

remove_failed_creations() {
  set +e
  if [[ "$CREATED_RELEASE" == "1" && -n "$RELEASE_DIR" ]]; then
    rm -rf -- "$RELEASE_DIR"
  fi
  if [[ "$CREATED_RELEASES_DIR" == "1" ]]; then rmdir -- /opt/pip-v2/releases; fi
  if [[ "$CREATED_OPT_DIR" == "1" ]]; then rmdir -- /opt/pip-v2; fi
  if [[ "$CREATED_ETC_DIR" == "1" ]]; then rmdir -- /etc/pip-v2; fi
  if [[ "$CREATED_USER" == "1" ]]; then
    rm -rf -- /var/lib/pip-v2
    userdel pip-v2-control >/dev/null 2>&1
  fi
  if [[ "$CREATED_GROUP" == "1" ]]; then
    groupdel pip-v2-control >/dev/null 2>&1
  fi
  set -e
}

finish() {
  local status=$?
  trap - EXIT
  if [[ "$status" -ne 0 && "$MUTATION_STARTED" == "1" ]]; then
    echo "installation failed; restoring the previous control-plane deployment" >&2
    rollback_install
  fi
  if [[ "$status" -ne 0 ]]; then
    remove_failed_creations
  fi
  rm -rf -- "$INSTALL_TMP"
  exit "$status"
}
trap finish EXIT

install -o root -g root -m 0600 "$WHEEL" "$INSTALL_TMP/input.whl"
ROOT_WHEEL="$INSTALL_TMP/input.whl"
WHEEL_SHA="$(sha256sum "$ROOT_WHEEL" | awk '{print $1}')"
[[ "$WHEEL_SHA" == "$EXPECTED_SHA" ]] || {
  echo "wheel SHA-256 mismatch" >&2
  exit 1
}
getent passwd "$CALLER" >/dev/null || { echo "unknown caller: $CALLER" >&2; exit 1; }
CALLER_UID="$(id -u "$CALLER")"
CALLER_GID="$(id -g "$CALLER")"
[[ "$CALLER_UID" != "0" ]] || { echo "root cannot be the control-plane caller" >&2; exit 1; }
CALLER_GROUP="$(id -gn "$CALLER")"
[[ "$CALLER_GROUP" =~ ^[a-zA-Z0-9_.-]+$ ]] || { echo "unsafe caller group" >&2; exit 1; }
CALLER_HOME="$(getent passwd "$CALLER" | awk -F: '{print $6}')"
[[ -d "$CALLER_HOME" && ! -L "$CALLER_HOME" ]] || {
  echo "caller home must be an existing non-symlink directory" >&2
  exit 1
}
CANONICAL_CALLER_HOME="$(realpath --canonicalize-existing -- "$CALLER_HOME")"
[[ "$CANONICAL_CALLER_HOME" == "$CALLER_HOME" ]] || {
  echo "caller home must equal its canonical real path" >&2
  exit 1
}
[[ "$(stat -c %u -- "$CALLER_HOME")" == "$CALLER_UID" ]] || {
  echo "caller home must be owned by the caller" >&2
  exit 1
}
/usr/bin/python3 -P - "$CALLER_HOME" <<'PY'
import sys
from pathlib import PurePosixPath

raw = sys.argv[1]
path = PurePosixPath(raw)
if (
    not path.is_absolute()
    or ".." in path.parts
    or str(path) != raw
    or any(character.isspace() for character in raw)
):
    raise SystemExit("caller home must be a canonical absolute path without whitespace")
PY
if ! getent group pip-v2-control >/dev/null; then
  groupadd --system pip-v2-control
  CREATED_GROUP=1
fi
if ! getent passwd pip-v2-control >/dev/null; then
  useradd --system --gid pip-v2-control --home-dir /nonexistent \
    --shell /usr/sbin/nologin --no-create-home pip-v2-control
  CREATED_USER=1
fi
CONTROL_UID="$(id -u pip-v2-control)"
CONTROL_GID="$(id -g pip-v2-control)"
EXPECTED_CONTROL_GID="$(getent group pip-v2-control | awk -F: '{print $3}')"
CONTROL_GROUPS="$(id -G pip-v2-control)"
CONTROL_HOME="$(getent passwd pip-v2-control | awk -F: '{print $6}')"
CONTROL_SHELL="$(getent passwd pip-v2-control | awk -F: '{print $7}')"
[[ "$CONTROL_UID" != "0" && "$CONTROL_UID" != "$CALLER_UID" && \
  "$CONTROL_GID" == "$EXPECTED_CONTROL_GID" && \
  "$CONTROL_GROUPS" == "$CONTROL_GID" && \
  "$CONTROL_HOME" == "/nonexistent" && \
  "$CONTROL_SHELL" == "/usr/sbin/nologin" ]] || {
  echo "pip-v2-control identity is not exclusively configured" >&2
  exit 1
}

mkdir -p "$INSTALL_TMP/app"
/usr/bin/python3 -P - "$ROOT_WHEEL" "$INSTALL_TMP/app" <<'PY'
import shutil
import stat
import sys
import zipfile
from pathlib import Path, PurePosixPath

wheel = Path(sys.argv[1])
target = Path(sys.argv[2]).resolve()
with zipfile.ZipFile(wheel) as archive:
    for item in archive.infolist():
        name = PurePosixPath(item.filename)
        if name.is_absolute() or ".." in name.parts:
            raise SystemExit(f"unsafe wheel member: {item.filename}")
        if stat.S_ISLNK(item.external_attr >> 16):
            raise SystemExit(f"wheel symlink is forbidden: {item.filename}")
        destination = target.joinpath(*name.parts)
        if item.is_dir():
            destination.mkdir(parents=True, exist_ok=True)
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        with archive.open(item) as source, destination.open("wb") as output:
            shutil.copyfileobj(source, output)
PY
export PYTHONDONTWRITEBYTECODE=1 PYTHONSAFEPATH=1
PYTHONPATH="$INSTALL_TMP/app" /usr/bin/python3 -P -m pip_agent.control_plane --help >/dev/null
PYTHONPATH="$INSTALL_TMP/app" /usr/bin/python3 -P -m pip_agent.control_plane \
  render-unit --caller-group "$CALLER_GROUP" \
  --output "$INSTALL_TMP/pip-v2-control.service"
PYTHONPATH="$INSTALL_TMP/app" /usr/bin/python3 -P -m pip_agent.control_plane \
  render-decision-units --caller-group "$CALLER_GROUP" \
  --service-output "$INSTALL_TMP/pip-v2-decision.service" \
  --timer-output "$INSTALL_TMP/pip-v2-decision.timer"
PYTHONPATH="$INSTALL_TMP/app" /usr/bin/python3 -P -m pip_agent.route_consumer \
  render-units --caller "$CALLER" --caller-group "$CALLER_GROUP" \
  --caller-home "$(getent passwd "$CALLER" | awk -F: '{print $6}')" \
  --service-output "$INSTALL_TMP/pip-v2-route-consumer.service" \
  --timer-output "$INSTALL_TMP/pip-v2-route-consumer.timer"

mkdir -p "$INSTALL_TMP/backup"
if [[ -e /opt/pip-v2/current || -L /opt/pip-v2/current ]]; then
  [[ -L /opt/pip-v2/current ]] || { echo "existing current path is not a symlink" >&2; exit 1; }
  OLD_TARGET="$(readlink /opt/pip-v2/current)"
  [[ "$OLD_TARGET" =~ ^releases/[0-9a-f]{64}$ ]] || {
    echo "existing current symlink has an unsafe target" >&2
    exit 1
  }
fi
systemctl is-enabled --quiet pip-v2-control.service 2>/dev/null && OLD_ENABLED=1
systemctl is-active --quiet pip-v2-control.service 2>/dev/null && OLD_ACTIVE=1
systemctl is-enabled --quiet pip-v2-decision.timer 2>/dev/null && OLD_TIMER_ENABLED=1
systemctl is-active --quiet pip-v2-decision.timer 2>/dev/null && OLD_TIMER_ACTIVE=1
systemctl is-enabled --quiet pip-v2-route-consumer.timer 2>/dev/null && OLD_ROUTER_TIMER_ENABLED=1
systemctl is-active --quiet pip-v2-route-consumer.timer 2>/dev/null && OLD_ROUTER_TIMER_ACTIVE=1
if [[ -e /usr/local/bin/pip-v2-control ]]; then cp -a /usr/local/bin/pip-v2-control "$INSTALL_TMP/backup/wrapper"; HAD_WRAPPER=1; fi
if [[ -e /usr/local/bin/pip-v2-route-consumer ]]; then cp -a /usr/local/bin/pip-v2-route-consumer "$INSTALL_TMP/backup/router-wrapper"; HAD_ROUTER_WRAPPER=1; fi
if [[ -e /etc/pip-v2/control.json ]]; then cp -a /etc/pip-v2/control.json "$INSTALL_TMP/backup/config"; HAD_CONFIG=1; fi
if [[ -e /etc/systemd/system/pip-v2-control.service ]]; then cp -a /etc/systemd/system/pip-v2-control.service "$INSTALL_TMP/backup/unit"; HAD_UNIT=1; fi
if [[ -e /etc/systemd/system/pip-v2-decision.service ]]; then cp -a /etc/systemd/system/pip-v2-decision.service "$INSTALL_TMP/backup/decision-unit"; HAD_DECISION_UNIT=1; fi
if [[ -e /etc/systemd/system/pip-v2-decision.timer ]]; then cp -a /etc/systemd/system/pip-v2-decision.timer "$INSTALL_TMP/backup/decision-timer"; HAD_DECISION_TIMER=1; fi
if [[ -e /etc/systemd/system/pip-v2-route-consumer.service ]]; then cp -a /etc/systemd/system/pip-v2-route-consumer.service "$INSTALL_TMP/backup/router-unit"; HAD_ROUTER_UNIT=1; fi
if [[ -e /etc/systemd/system/pip-v2-route-consumer.timer ]]; then cp -a /etc/systemd/system/pip-v2-route-consumer.timer "$INSTALL_TMP/backup/router-timer"; HAD_ROUTER_TIMER=1; fi
if [[ -e /opt/pip-v2/WHEEL.SHA256 ]]; then cp -a /opt/pip-v2/WHEEL.SHA256 "$INSTALL_TMP/backup/wheel-sha"; HAD_WHEEL_SHA=1; fi

prepare_system_dir() {
  local path="$1" created_var="$2"
  if [[ -e "$path" || -L "$path" ]]; then
    [[ -d "$path" && ! -L "$path" && "$(stat -c %u:%g:%a -- "$path")" == "0:0:755" ]] || {
      echo "refusing unsafe preexisting system directory: $path" >&2
      return 1
    }
  else
    install -d -o root -g root -m 0755 -- "$path"
    printf -v "$created_var" '%s' 1
  fi
}

MUTATION_STARTED=1
prepare_system_dir /opt/pip-v2 CREATED_OPT_DIR
prepare_system_dir /opt/pip-v2/releases CREATED_RELEASES_DIR
prepare_system_dir /etc/pip-v2 CREATED_ETC_DIR
RELEASE_DIR="/opt/pip-v2/releases/$WHEEL_SHA"
find "$INSTALL_TMP/app" -type d -exec chmod 0755 {} +
find "$INSTALL_TMP/app" -type f -exec chmod 0644 {} +
if [[ -e "$RELEASE_DIR" || -L "$RELEASE_DIR" ]]; then
  [[ -d "$RELEASE_DIR" && ! -L "$RELEASE_DIR" ]] || {
    echo "existing content-addressed release is unsafe" >&2
    exit 1
  }
  [[ -z "$(find "$RELEASE_DIR" -type l -print -quit)" ]] || {
    echo "existing content-addressed release contains a symlink" >&2
    exit 1
  }
  [[ -z "$(find "$RELEASE_DIR" \( \! -user root -o \! -group root \) -print -quit)" ]] || {
    echo "existing content-addressed release has unsafe ownership" >&2
    exit 1
  }
  while IFS= read -r -d '' path; do
    [[ "$(stat -c %a -- "$path")" == "755" ]] || { echo "unsafe release directory mode" >&2; exit 1; }
  done < <(find "$RELEASE_DIR" -type d -print0)
  while IFS= read -r -d '' path; do
    [[ "$(stat -c %a -- "$path")" == "644" ]] || { echo "unsafe release file mode" >&2; exit 1; }
  done < <(find "$RELEASE_DIR" -type f -print0)
  diff -qr --no-dereference "$INSTALL_TMP/app" "$RELEASE_DIR" >/dev/null || {
    echo "existing content-addressed release does not match the verified wheel" >&2
    exit 1
  }
else
  mv "$INSTALL_TMP/app" "$RELEASE_DIR"
  CREATED_RELEASE=1
fi

ln -sfn "releases/$WHEEL_SHA" /opt/pip-v2/current.new
mv -Tf /opt/pip-v2/current.new /opt/pip-v2/current
printf '%s\n' "$WHEEL_SHA" > /opt/pip-v2/WHEEL.SHA256

LINK_RESULT="$(runuser -u "$CALLER" -- /usr/bin/env \
  HOME="$CALLER_HOME" \
  PATH="$CALLER_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin" \
  PYTHONPATH=/opt/pip-v2/current \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONSAFEPATH=1 \
  /usr/bin/python3 -P -m pip_agent.bootstrap \
  --repo-root /opt/pip-v2/current/pip_agent/resources \
  --hermes-home "$CALLER_HOME/.hermes" \
  --link-cursor-roles)"
/usr/bin/python3 -P - "$LINK_RESULT" "$CALLER_HOME" > "$INSTALL_TMP/created-links" <<'PY'
import json
import sys
from pathlib import Path

payload = json.loads(sys.argv[1])
home = Path(sys.argv[2])
allowed = {
    home / ".hermes/profiles/cursor-fixer/skills/builder-grok",
    home / ".hermes/profiles/cursor-fixer/skills/workflow-contract",
    home / ".hermes/profiles/cursor-reviewer/skills/reviewer-secperf",
    home / ".hermes/profiles/cursor-reviewer/skills/workflow-contract",
}
created = {Path(value) for value in payload["created"]}
if not created <= allowed:
    raise SystemExit("bootstrap returned an unsafe created-link path")
for path in sorted(created, key=str):
    print(path)
PY
mapfile -t CREATED_CURSOR_LINKS < "$INSTALL_TMP/created-links"
chmod 0711 "$INSTALL_TMP"
install -d -o "$CALLER" -g "$CALLER_GROUP" -m 0700 "$INSTALL_TMP/profile-snapshot"
runuser -u "$CALLER" -- /usr/bin/python3 -P - \
  "$INSTALL_TMP/profile-snapshot/state.json" "$CALLER_HOME" <<'PY'
import base64
import json
import os
import sys
from pathlib import Path

output = Path(sys.argv[1])
profiles = Path(sys.argv[2]) / ".hermes/profiles"
state = {}
for role in ("planner", "reviewer-general", "final-reviewer"):
    profile = profiles / role
    if profile.is_symlink() or (profile.exists() and not profile.is_dir()):
        raise SystemExit(f"refusing to snapshot unsafe profile root: {profile}")
    skills = profile / "skills"
    if skills.is_symlink() or (skills.exists() and not skills.is_dir()):
        raise SystemExit(f"refusing to snapshot unsafe skills directory: {skills}")
    skills_state = {
        "existed": skills.is_dir(),
        "mode": (skills.stat().st_mode & 0o777) if skills.is_dir() else None,
    }
    entries = {}
    for relative in (
        "config.yaml",
        "pip-v2-profile.json",
        f"skills/{role}",
        "skills/workflow-contract",
        "auth.json",
        "auth.lock",
    ):
        path = profile / relative
        if path.is_symlink() and relative in {"config.yaml", "pip-v2-profile.json"}:
            raise SystemExit(f"refusing to snapshot symlinked profile file: {path}")
        if path.is_symlink():
            entries[relative] = {"type": "symlink", "target": os.readlink(path)}
        elif path.is_file() and relative in {"config.yaml", "pip-v2-profile.json"}:
            entries[relative] = {
                "type": "file",
                "data": base64.b64encode(path.read_bytes()).decode(),
                "mode": path.stat().st_mode & 0o777,
            }
        elif path.exists():
            raise SystemExit(f"refusing to snapshot unexpected profile path: {path}")
        else:
            entries[relative] = None
    state[role] = {
        "existed": profile.is_dir(),
        "mode": (profile.stat().st_mode & 0o777) if profile.is_dir() else None,
        "skills": skills_state,
        "entries": entries,
    }
output.write_text(json.dumps(state, sort_keys=True) + "\n")
os.chmod(output, 0o600)
PY
PROFILE_SNAPSHOT_READY=1
runuser -u "$CALLER" -- /usr/bin/env \
  HOME="$CALLER_HOME" \
  PATH="$CALLER_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin" \
  PYTHONPATH=/opt/pip-v2/current \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONSAFEPATH=1 \
  /usr/bin/python3 -P -m pip_agent.bootstrap \
  --repo-root /opt/pip-v2/current/pip_agent/resources \
  --hermes-home "$CALLER_HOME/.hermes" \
  --apply >/dev/null
BOARD_RESULT="$(runuser -u "$CALLER" -- /usr/bin/env \
  HOME="$CALLER_HOME" \
  PATH="$CALLER_HOME/.local/bin:/usr/local/bin:/usr/bin:/bin" \
  PYTHONPATH=/opt/pip-v2/current \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONSAFEPATH=1 \
  /usr/bin/python3 -P -m pip_agent.bootstrap \
  --repo-root /opt/pip-v2/current/pip_agent/resources \
  --hermes-home "$CALLER_HOME/.hermes" \
  --ensure-board pip-mdk)"
if /usr/bin/python3 -c 'import json,sys; raise SystemExit(not json.loads(sys.argv[1])["created"])' "$BOARD_RESULT"; then
  CREATED_BOARD=1
fi
chmod 0644 /opt/pip-v2/WHEEL.SHA256

cat > "$INSTALL_TMP/pip-v2-control" <<'SH'
#!/bin/sh
export PYTHONPATH=/opt/pip-v2/current
export PYTHONDONTWRITEBYTECODE=1
export PYTHONSAFEPATH=1
exec /usr/bin/python3 -P -m pip_agent.control_plane "$@"
SH
install -o root -g root -m 0755 "$INSTALL_TMP/pip-v2-control" /usr/local/bin/pip-v2-control

cat > "$INSTALL_TMP/pip-v2-route-consumer" <<'SH'
#!/bin/sh
export PYTHONPATH=/opt/pip-v2/current
export PYTHONDONTWRITEBYTECODE=1
export PYTHONSAFEPATH=1
exec /usr/bin/python3 -P -m pip_agent.route_consumer "$@"
SH
install -o root -g root -m 0755 "$INSTALL_TMP/pip-v2-route-consumer" /usr/local/bin/pip-v2-route-consumer

/usr/bin/python3 -P - "$INSTALL_TMP/control.json" "$ISSUE" "$CALLER_UID" "$CALLER_GID" <<'PY'
import json
import sys
from pathlib import Path

path, issue, caller_uid, caller_gid = sys.argv[1:]
payload = {
    "repository": "marmot-protocol/mdk",
    "issue_number": int(issue),
    "intake_label": "pip-ok",
    "merge_mode": "shadow",
    "autonomous_merge": False,
    "state_database": "/var/lib/pip-v2/cases.db",
    "socket_path": "/run/pip-v2/control.sock",
    "socket_group": int(caller_gid),
    "allowed_uids": [int(caller_uid)],
}
Path(path).write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
PY
install -o root -g pip-v2-control -m 0640 "$INSTALL_TMP/control.json" /etc/pip-v2/control.json
chmod 0640 /etc/pip-v2/control.json

install -o root -g root -m 0644 "$INSTALL_TMP/pip-v2-control.service" \
  /etc/systemd/system/pip-v2-control.service
install -o root -g root -m 0644 "$INSTALL_TMP/pip-v2-decision.service" \
  /etc/systemd/system/pip-v2-decision.service
install -o root -g root -m 0644 "$INSTALL_TMP/pip-v2-decision.timer" \
  /etc/systemd/system/pip-v2-decision.timer
install -o root -g root -m 0644 "$INSTALL_TMP/pip-v2-route-consumer.service" \
  /etc/systemd/system/pip-v2-route-consumer.service
install -o root -g root -m 0644 "$INSTALL_TMP/pip-v2-route-consumer.timer" \
  /etc/systemd/system/pip-v2-route-consumer.timer

systemctl daemon-reload
systemctl enable pip-v2-control.service
systemctl restart pip-v2-control.service
systemctl is-active --quiet pip-v2-control.service
runuser -u "$CALLER" -- /usr/local/bin/pip-v2-control request \
  --socket /run/pip-v2/control.sock --operation ensure_canary >/dev/null
systemctl enable pip-v2-decision.timer
systemctl start pip-v2-decision.service
systemctl restart pip-v2-decision.timer
systemctl is-active --quiet pip-v2-decision.timer
systemctl enable pip-v2-route-consumer.timer
systemctl restart pip-v2-route-consumer.timer
systemctl is-active --quiet pip-v2-route-consumer.timer

fail_closed() {
  echo "control-plane boundary validation failed" >&2
  return 1
}
[[ "$(stat -c '%U:%G:%a' /var/lib/pip-v2)" == \
  "pip-v2-control:pip-v2-control:700" ]] || fail_closed
[[ "$(stat -c '%U:%G:%a' /var/lib/pip-v2/cases.db)" == \
  "pip-v2-control:pip-v2-control:600" ]] || fail_closed
[[ "$(stat -c '%U:%g:%a' /run/pip-v2/control.sock)" == \
  "pip-v2-control:${CALLER_GID}:660" ]] || fail_closed
[[ "$(stat -c '%U:%g:%a' /run/pip-v2/decision-route.json)" == \
  "pip-v2-control:${CALLER_GID}:640" ]] || fail_closed
if runuser -u "$CALLER" -- test -e /var/lib/pip-v2/cases.db; then
  fail_closed
fi
runuser -u "$CALLER" -- /usr/local/bin/pip-v2-control request \
  --socket /run/pip-v2/control.sock --operation status >/dev/null || fail_closed

MUTATION_STARTED=0
printf 'installed and validated pip-v2-control for marmot-protocol/mdk#%s; caller=%s\n' "$ISSUE" "$CALLER"
