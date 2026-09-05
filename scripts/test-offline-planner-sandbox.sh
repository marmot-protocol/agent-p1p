#!/usr/bin/env bash
# Linux host integration probe. No auth copy, model process, network or live board.
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
sudo -n true
sudo -n mountpoint -q /var/lib/pip/worktrees
if ! sudo -n test -e /var/lib/pip/worktrees/hermes-scratch; then
  sudo -n install -d -o pip-control -g pip-control -m 0700 /var/lib/pip/worktrees/hermes-scratch
fi
test "$(sudo -n stat -c '%U:%G:%a' /var/lib/pip/worktrees/hermes-scratch)" = pip-control:pip-control:700
scratch=$(sudo -n -u pip-control mktemp -d /var/lib/pip/worktrees/hermes-scratch/offline-XXXXXXXX)
unit="pip-offline-sandbox-${scratch##*/}"
sudo -n install -d -o pip-control -g pip-control -m 0700 "$scratch/source" "$scratch/source/src"
for file in Cargo.toml Cargo.lock rust-toolchain.toml; do
  sudo -n install -o root -g root -m 0444 "$repo_root/tests/fixtures/offline-sandbox/$file" "$scratch/source/$file"
done
sudo -n install -o root -g root -m 0444 "$repo_root/tests/fixtures/offline-sandbox/src/lib.rs" "$scratch/source/src/lib.rs"
sudo -n install -o root -g root -m 0444 "$repo_root/tests/fixtures/offline_planner_sandbox.py" "$scratch/source/"
sudo -n install -o root -g root -m 0444 "$repo_root/migration/target-v1/worker-results.json" "$scratch/source/"
sudo -n install -o root -g root -m 0444 "$repo_root/tests/fixtures/offline-sandbox/OFFLINE_PROBE" "$scratch/OFFLINE_PROBE"
properties=()
while IFS= read -r line; do
  case "$line" in
    UMask=*|NoNewPrivileges=*|PrivateDevices=*|PrivateTmp=*|ProtectClock=*|ProtectControlGroups=*|ProtectHome=*|ProtectHostname=*|ProtectKernelLogs=*|ProtectKernelModules=*|ProtectKernelTunables=*|ProtectSystem=*|RestrictNamespaces=*|RestrictRealtime=*|RestrictSUIDSGID=*|SystemCallArchitectures=*|LockPersonality=*|MemoryDenyWriteExecute=*|CapabilityBoundingSet=*|AmbientCapabilities=*) properties+=(--property="$line") ;;
  esac
done < "$repo_root/packaging/systemd/pip-hermes-gateway.service"
sudo -n systemd-run --unit="$unit" --wait --pipe \
  --property=User=pip-control --property=Group=pip-control \
  --property="WorkingDirectory=$scratch" \
  --property="ReadOnlyPaths=/opt/pip/current /var/lib/pip/repositories /var/lib/pip/worktrees $scratch/source" \
  --property="ReadWritePaths=$scratch" \
  --property="InaccessiblePaths=/var/lib/pip/ledger.db /var/lib/pip/artifacts /var/lib/pip/provider-home /var/lib/pip/hermes" \
  --property=RestrictAddressFamilies=AF_UNIX --property=IPAddressDeny=any \
  --property=RuntimeMaxSec=180 --property=MemoryMax=2G --property=TasksMax=128 \
  --setenv=PATH=/usr/local/bin:/usr/bin:/bin --setenv=PYTHONPATH=/usr/local/lib/hermes-agent \
  "${properties[@]}" \
  /usr/local/lib/hermes-agent/venv/bin/python "$scratch/source/offline_planner_sandbox.py" "$scratch/source" "$scratch" "$scratch/source/worker-results.json"
printf 'Report: %s/results/report.json\n' "$scratch"
printf 'Retained isolated probe: %s\n' "$scratch"
