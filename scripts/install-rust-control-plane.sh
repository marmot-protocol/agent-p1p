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

exec 9>/run/lock/pip-install.lock
flock -n 9 || { echo "another Pip installation is active" >&2; exit 1; }

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

staging=$(mktemp -d /var/tmp/pip-install.XXXXXX)
created_paths=()
control_user_created=false
worker_user_created=false
ingress_user_created=false
bootstrap_workspace_adopted=false
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
    if [[ $bootstrap_workspace_adopted == true ]]; then
      chown root:root /var/lib/pip/worktrees /var/lib/pip 2>/dev/null || true
      chmod 0770 /var/lib/pip/worktrees 2>/dev/null || true
      chmod 0755 /var/lib/pip 2>/dev/null || true
    fi
    if [[ $worker_user_created == true && $cleanup_safe == true ]]; then
      userdel pip-worker >/dev/null 2>&1 || true
    fi
    if [[ $ingress_user_created == true && $cleanup_safe == true ]]; then
      userdel pip-ingress >/dev/null 2>&1 || true
      groupdel pip-ingress >/dev/null 2>&1 || true
    fi
    if [[ $control_user_created == true && $cleanup_safe == true ]]; then
      userdel pip-control >/dev/null 2>&1 || true
      groupdel pip-control >/dev/null 2>&1 || true
    elif [[ $control_user_created == true ]]; then
      echo "retained pip-control because failed-install artifacts remain" >&2
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

if ! getent passwd pip-control >/dev/null; then
  useradd --system --user-group --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin pip-control
  control_user_created=true
fi

passwd_entry=$(getent passwd pip-control)
IFS=: read -r account _ control_uid control_gid _ control_home control_shell <<<"$passwd_entry"
[[ $account == pip-control && $control_uid != 0 && $control_home == /nonexistent ]] || {
  echo "pip-control identity has an unsafe account definition" >&2
  exit 1
}
[[ $control_shell == /usr/sbin/nologin || $control_shell == /sbin/nologin ]] || {
  echo "pip-control must use a nologin shell" >&2
  exit 1
}
[[ ! -e $control_home ]] || { echo "pip-control home must not exist" >&2; exit 1; }
[[ $(id -G pip-control) == "$control_gid" ]] || {
  echo "pip-control must not belong to supplementary groups" >&2
  exit 1
}
group_entry=$(getent group "$control_gid")
IFS=: read -r control_group _ resolved_gid members <<<"$group_entry"
[[ $control_group == pip-control && $resolved_gid == "$control_gid" && -z $members ]] || {
  echo "pip-control primary group is unsafe" >&2
  exit 1
}

if ! getent passwd pip-worker >/dev/null; then
  useradd --system --gid pip-control --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin pip-worker
  worker_user_created=true
fi
worker_entry=$(getent passwd pip-worker)
IFS=: read -r worker_account _ worker_uid worker_gid _ worker_home worker_shell <<<"$worker_entry"
[[ $worker_account == pip-worker && $worker_uid != 0 && $worker_uid != "$control_uid" &&
   $worker_gid == "$control_gid" && $worker_home == /nonexistent ]] || {
  echo "pip-worker identity has an unsafe account definition" >&2
  exit 1
}
[[ $worker_shell == /usr/sbin/nologin || $worker_shell == /sbin/nologin ]] || {
  echo "pip-worker must use a nologin shell" >&2
  exit 1
}
[[ $(id -G pip-worker) == "$control_gid" ]] || {
  echo "pip-worker must have only the control group" >&2
  exit 1
}

if ! getent passwd pip-ingress >/dev/null; then
  useradd --system --user-group --no-create-home --home-dir /nonexistent \
    --shell /usr/sbin/nologin pip-ingress
  ingress_user_created=true
fi
ingress_entry=$(getent passwd pip-ingress)
IFS=: read -r ingress_account _ ingress_uid ingress_gid _ ingress_home ingress_shell <<<"$ingress_entry"
[[ $ingress_account == pip-ingress && $ingress_uid != 0 && $ingress_uid != "$control_uid" &&
   $ingress_uid != "$worker_uid" && $ingress_home == /nonexistent ]] || {
  echo "pip-ingress identity has an unsafe account definition" >&2
  exit 1
}
[[ $ingress_shell == /usr/sbin/nologin || $ingress_shell == /sbin/nologin ]] || {
  echo "pip-ingress must use a nologin shell" >&2
  exit 1
}
[[ $(id -G pip-ingress) == "$ingress_gid" ]] || {
  echo "pip-ingress must not belong to supplementary groups" >&2
  exit 1
}
ingress_group_entry=$(getent group "$ingress_gid")
IFS=: read -r ingress_group _ resolved_ingress_gid ingress_members <<<"$ingress_group_entry"
[[ $ingress_group == pip-ingress && $resolved_ingress_gid == "$ingress_gid" && -z $ingress_members ]] || {
  echo "pip-ingress primary group is unsafe" >&2
  exit 1
}

adopt_bootstrap_workspace_layout() {
  local state_root=/var/lib/pip
  local worktree_root=/var/lib/pip/worktrees

  [[ -e $state_root || -L $state_root ]] || return 0
  [[ $(stat -c '%u:%g:%a' "$state_root") == 0:0:755 ]] || return 0

  [[ -d $state_root && ! -L $state_root && -d $worktree_root && ! -L $worktree_root ]] || {
    echo "unsafe bootstrap workspace layout" >&2
    exit 1
  }
  [[ $(stat -c '%u:%g:%a' "$worktree_root") == 0:0:770 ]] || {
    echo "bootstrap worktree directory has unexpected ownership or mode" >&2
    exit 1
  }
  mountpoint -q "$worktree_root" || {
    echo "bootstrap worktree directory must be a mount point" >&2
    exit 1
  }
  [[ $(stat -c '%d' "$state_root") != $(stat -c '%d' "$worktree_root") ]] || {
    echo "bootstrap worktree storage must use a distinct filesystem" >&2
    exit 1
  }
  [[ $(find "$state_root" -mindepth 1 -maxdepth 1 -printf '%f\n') == worktrees ]] || {
    echo "bootstrap state directory contains unexpected entries" >&2
    exit 1
  }
  if find "$worktree_root" -mindepth 1 -print -quit | grep -q .; then
    echo "bootstrap worktree directory must be empty" >&2
    exit 1
  fi

  chown "$control_uid:$control_gid" "$worktree_root" "$state_root"
  chmod 0770 "$worktree_root"
  chmod 0710 "$state_root"
  bootstrap_workspace_adopted=true
}

adopt_bootstrap_workspace_layout

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

ensure_directory /opt/pip root root 755
ensure_directory /opt/pip/releases root root 755
ensure_directory /etc/pip root root 755
ensure_directory /etc/pip/repositories root root 755
ensure_directory /etc/systemd/system root root 755
ensure_directory /var/lib/pip pip-control pip-control 710
ensure_directory /var/lib/pip/repositories pip-control pip-control 700
ensure_directory /var/lib/pip/worktrees pip-control pip-control 770
ensure_directory /var/lib/pip/worktrees/hermes-scratch pip-control pip-control 700
ensure_directory /var/lib/pip/artifacts pip-control pip-control 770
ensure_directory /var/lib/pip/provider-home pip-worker pip-control 700
ensure_directory /var/lib/pip/hermes pip-control pip-control 700
ensure_directory /var/lib/pip/direct-queue pip-control pip-control 750
ensure_directory /var/lib/pip/direct-queue/inbox pip-control pip-control 750
ensure_directory /var/lib/pip/direct-queue/results pip-worker pip-control 770
ensure_directory /var/lib/pip/direct-queue/archive pip-control pip-control 700
ensure_directory /var/spool/pip-webhooks root root 711
ensure_directory /var/spool/pip-webhooks/receipts pip-ingress pip-control 2750
ensure_directory /var/spool/pip-webhooks/pending pip-ingress pip-control 2770
ensure_directory /var/spool/pip-webhooks/processed pip-control pip-control 711

"$staging/cohort/root/bin/pip-control" install-release \
  --cohort "$staging/cohort" \
  --public-key "$staging/release-public.key" \
  --manifest-sha256 "$manifest_sha256" \
  --binary-sha256 "$binary_sha256" \
  --systemctl "$systemctl_path" \
  --state-uid "$control_uid" \
  --state-gid "$control_gid" \
  --install-root /opt/pip \
  --config-root /etc/pip \
  --unit-root /etc/systemd/system \
  --state-root /var/lib/pip

installation_complete=true
echo '{"ok":true,"policy_state":"preserved_or_seeded_paused","timer_state":"preserved"}'
