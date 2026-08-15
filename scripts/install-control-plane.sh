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
HAD_WRAPPER=0
HAD_CONFIG=0
HAD_UNIT=0
HAD_WHEEL_SHA=0
CREATED_GROUP=0
CREATED_USER=0
CREATED_RELEASE=0
RELEASE_DIR=""

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
  systemctl stop pip-v2-control.service >/dev/null 2>&1
  if [[ -n "$OLD_TARGET" ]]; then
    ln -sfn "$OLD_TARGET" /opt/pip-v2/current.rollback
    mv -Tf /opt/pip-v2/current.rollback /opt/pip-v2/current
  else
    rm -f /opt/pip-v2/current
  fi
  restore_file "$HAD_WRAPPER" "$INSTALL_TMP/backup/wrapper" /usr/local/bin/pip-v2-control
  restore_file "$HAD_CONFIG" "$INSTALL_TMP/backup/config" /etc/pip-v2/control.json
  restore_file "$HAD_UNIT" "$INSTALL_TMP/backup/unit" /etc/systemd/system/pip-v2-control.service
  restore_file "$HAD_WHEEL_SHA" "$INSTALL_TMP/backup/wheel-sha" /opt/pip-v2/WHEEL.SHA256
  systemctl daemon-reload >/dev/null 2>&1
  if [[ "$OLD_ENABLED" == "1" ]]; then
    systemctl enable pip-v2-control.service >/dev/null 2>&1
  else
    systemctl disable pip-v2-control.service >/dev/null 2>&1
  fi
  if [[ "$OLD_ACTIVE" == "1" ]]; then
    systemctl start pip-v2-control.service >/dev/null 2>&1
  fi
  set -e
}

remove_failed_creations() {
  set +e
  if [[ "$CREATED_RELEASE" == "1" && -n "$RELEASE_DIR" ]]; then
    rm -rf -- "$RELEASE_DIR"
  fi
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
PYTHONPATH="$INSTALL_TMP/app" /usr/bin/python3 -P -m pip_agent.control_plane --help >/dev/null
PYTHONPATH="$INSTALL_TMP/app" /usr/bin/python3 -P -m pip_agent.control_plane \
  render-unit --caller-group "$CALLER_GROUP" \
  --output "$INSTALL_TMP/pip-v2-control.service"

install -d -o root -g root -m 0755 /opt/pip-v2 /opt/pip-v2/releases /etc/pip-v2
RELEASE_DIR="/opt/pip-v2/releases/$WHEEL_SHA"
if [[ ! -d "$RELEASE_DIR" ]]; then
  mv "$INSTALL_TMP/app" "$RELEASE_DIR"
  CREATED_RELEASE=1
fi
chown -R root:root "$RELEASE_DIR"
find "$RELEASE_DIR" -type d -exec chmod 0755 {} +
find "$RELEASE_DIR" -type f -exec chmod 0644 {} +

mkdir -p "$INSTALL_TMP/backup"
OLD_TARGET="$(readlink /opt/pip-v2/current 2>/dev/null || true)"
systemctl is-enabled --quiet pip-v2-control.service 2>/dev/null && OLD_ENABLED=1
systemctl is-active --quiet pip-v2-control.service 2>/dev/null && OLD_ACTIVE=1
if [[ -e /usr/local/bin/pip-v2-control ]]; then cp -a /usr/local/bin/pip-v2-control "$INSTALL_TMP/backup/wrapper"; HAD_WRAPPER=1; fi
if [[ -e /etc/pip-v2/control.json ]]; then cp -a /etc/pip-v2/control.json "$INSTALL_TMP/backup/config"; HAD_CONFIG=1; fi
if [[ -e /etc/systemd/system/pip-v2-control.service ]]; then cp -a /etc/systemd/system/pip-v2-control.service "$INSTALL_TMP/backup/unit"; HAD_UNIT=1; fi
if [[ -e /opt/pip-v2/WHEEL.SHA256 ]]; then cp -a /opt/pip-v2/WHEEL.SHA256 "$INSTALL_TMP/backup/wheel-sha"; HAD_WHEEL_SHA=1; fi
MUTATION_STARTED=1

ln -sfn "releases/$WHEEL_SHA" /opt/pip-v2/current.new
mv -Tf /opt/pip-v2/current.new /opt/pip-v2/current
printf '%s\n' "$WHEEL_SHA" > /opt/pip-v2/WHEEL.SHA256
chmod 0644 /opt/pip-v2/WHEEL.SHA256

cat > "$INSTALL_TMP/pip-v2-control" <<'SH'
#!/bin/sh
export PYTHONPATH=/opt/pip-v2/current
export PYTHONDONTWRITEBYTECODE=1
export PYTHONSAFEPATH=1
exec /usr/bin/python3 -P -m pip_agent.control_plane "$@"
SH
install -o root -g root -m 0755 "$INSTALL_TMP/pip-v2-control" /usr/local/bin/pip-v2-control

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

systemctl daemon-reload
systemctl enable pip-v2-control.service
systemctl restart pip-v2-control.service
systemctl is-active --quiet pip-v2-control.service

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
if runuser -u "$CALLER" -- test -e /var/lib/pip-v2/cases.db; then
  fail_closed
fi
runuser -u "$CALLER" -- /usr/local/bin/pip-v2-control request \
  --socket /run/pip-v2/control.sock --operation status >/dev/null || fail_closed

MUTATION_STARTED=0
printf 'installed and validated pip-v2-control for marmot-protocol/mdk#%s; caller=%s\n' "$ISSUE" "$CALLER"
