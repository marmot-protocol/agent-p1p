#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --cohort PATH --public-key PATH --manifest-sha256 HEX --binary-sha256 HEX" >&2
  exit 2
}

cohort=""
public_key=""
manifest_sha256=""
binary_sha256=""
while (($#)); do
  case "$1" in
    --cohort) cohort=${2-}; shift 2 ;;
    --public-key) public_key=${2-}; shift 2 ;;
    --manifest-sha256) manifest_sha256=${2-}; shift 2 ;;
    --binary-sha256) binary_sha256=${2-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$cohort" && -n "$public_key" && -n "$manifest_sha256" && -n "$binary_sha256" ]] || usage
[[ $manifest_sha256 =~ ^[0-9a-f]{64}$ && $binary_sha256 =~ ^[0-9a-f]{64}$ ]] || {
  echo "release digests must be 64 lowercase hexadecimal characters" >&2
  exit 2
}
[[ $(id -u) -eq 0 ]] || { echo "installer must run as root" >&2; exit 1; }

exec 9>/run/lock/pip-v2-install.lock
flock -n 9 || { echo "another Pip v2 installation is active" >&2; exit 1; }

[[ -d "$cohort/root" && ! -L "$cohort" && ! -L "$cohort/root" ]] || {
  echo "cohort must contain a real root directory" >&2
  exit 1
}
for file in "$cohort/release-manifest.json" "$cohort/release-manifest.sig" "$public_key"; do
  [[ -f "$file" && ! -L "$file" ]] || { echo "unsafe installer input: $file" >&2; exit 1; }
done
[[ $(stat -c '%u' "$public_key") -eq 0 ]] || { echo "public key must be root-owned" >&2; exit 1; }
key_mode=$(stat -c '%a' "$public_key")
[[ $key_mode == 400 || $key_mode == 440 || $key_mode == 444 ]] || {
  echo "public key mode must be 0400, 0440, or 0444" >&2
  exit 1
}
systemctl_path=$(command -v systemctl)
[[ -n "$systemctl_path" && -x "$systemctl_path" ]] || { echo "systemctl is unavailable" >&2; exit 1; }

staging=$(mktemp -d /var/tmp/pip-v2-install.XXXXXX)
created_paths=()
control_user_created=false
installation_complete=false
cleanup() {
  status=$?
  if [[ -n ${staging:-} && -d $staging ]]; then
    rm -rf -- "$staging"
  fi
  if [[ $installation_complete != true ]]; then
    cleanup_safe=true
    for ((index=${#created_paths[@]} - 1; index >= 0; index--)); do
      if ! rmdir -- "${created_paths[$index]}" 2>/dev/null; then
        cleanup_safe=false
      fi
    done
    if [[ $control_user_created == true && $cleanup_safe == true ]]; then
      userdel pip-v2-control >/dev/null 2>&1 || true
      groupdel pip-v2-control >/dev/null 2>&1 || true
    elif [[ $control_user_created == true ]]; then
      echo "retained pip-v2-control because failed-install artifacts remain" >&2
    fi
  fi
  exit "$status"
}
trap cleanup EXIT

install -d -o root -g root -m 0700 "$staging/cohort"
cp -a -- "$cohort/." "$staging/cohort/"
if find "$staging/cohort" \( -type l -o \! -type f -a \! -type d \) -print -quit | grep -q .; then
  echo "release cohort contains a symlink or special file" >&2
  exit 1
fi
chown -R root:root "$staging/cohort"
install -o root -g root -m 0400 "$public_key" "$staging/release-public.key"

actual_manifest_sha256=$(sha256sum "$staging/cohort/release-manifest.json" | awk '{print $1}')
actual_binary_sha256=$(sha256sum "$staging/cohort/root/bin/pip-control" | awk '{print $1}')
[[ $actual_manifest_sha256 == "$manifest_sha256" ]] || {
  echo "staged manifest SHA-256 does not match the pre-root verified digest" >&2
  exit 1
}
[[ $actual_binary_sha256 == "$binary_sha256" ]] || {
  echo "staged binary SHA-256 does not match the pre-root verified digest" >&2
  exit 1
}

if ! getent passwd pip-v2-control >/dev/null; then
  useradd --system --user-group --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin pip-v2-control
  control_user_created=true
fi

passwd_entry=$(getent passwd pip-v2-control)
IFS=: read -r account _ control_uid control_gid _ control_home control_shell <<<"$passwd_entry"
[[ $account == pip-v2-control && $control_uid != 0 && $control_home == /nonexistent ]] || {
  echo "pip-v2-control identity has an unsafe account definition" >&2
  exit 1
}
[[ $control_shell == /usr/sbin/nologin || $control_shell == /sbin/nologin ]] || {
  echo "pip-v2-control must use a nologin shell" >&2
  exit 1
}
[[ ! -e $control_home ]] || { echo "pip-v2-control home must not exist" >&2; exit 1; }
[[ $(id -G pip-v2-control) == "$control_gid" ]] || {
  echo "pip-v2-control must not belong to supplementary groups" >&2
  exit 1
}
group_entry=$(getent group "$control_gid")
IFS=: read -r control_group _ resolved_gid members <<<"$group_entry"
[[ $control_group == pip-v2-control && $resolved_gid == "$control_gid" && -z $members ]] || {
  echo "pip-v2-control primary group is unsafe" >&2
  exit 1
}

ensure_directory() {
  path=$1
  owner=$2
  group=$3
  mode=$4
  if [[ -e $path || -L $path ]]; then
    [[ -d $path && ! -L $path ]] || { echo "unsafe installation directory: $path" >&2; exit 1; }
    [[ $(stat -c '%U:%G:%a' "$path") == "$owner:$group:$mode" ]] || {
      echo "installation directory has unexpected ownership or mode: $path" >&2
      exit 1
    }
  else
    install -d -o "$owner" -g "$group" -m "0$mode" "$path"
    created_paths+=("$path")
  fi
}

ensure_directory /opt/pip-v2 root root 755
ensure_directory /opt/pip-v2/releases root root 755
ensure_directory /etc/pip-v2 root root 755
ensure_directory /etc/pip-v2/repositories root root 755
ensure_directory /etc/systemd/system root root 755
ensure_directory /var/lib/pip-v2 pip-v2-control pip-v2-control 700
ensure_directory /var/lib/pip-v2/repositories pip-v2-control pip-v2-control 700
ensure_directory /var/lib/pip-v2/worktrees pip-v2-control pip-v2-control 700
ensure_directory /var/lib/pip-v2/artifacts pip-v2-control pip-v2-control 700
ensure_directory /var/lib/pip-v2/provider-home pip-v2-control pip-v2-control 700
ensure_directory /var/lib/pip-v2/hermes pip-v2-control pip-v2-control 700

"$staging/cohort/root/bin/pip-control" install-release \
  --cohort "$staging/cohort" \
  --public-key "$staging/release-public.key" \
  --manifest-sha256 "$manifest_sha256" \
  --binary-sha256 "$binary_sha256" \
  --systemctl "$systemctl_path" \
  --state-uid "$control_uid" \
  --state-gid "$control_gid" \
  --install-root /opt/pip-v2 \
  --config-root /etc/pip-v2 \
  --unit-root /etc/systemd/system \
  --state-root /var/lib/pip-v2

installation_complete=true
echo '{"ok":true,"intake_enabled":false,"dispatch_enabled":false,"timer_state":"preserved"}'
