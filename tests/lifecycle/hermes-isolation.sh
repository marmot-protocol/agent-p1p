#!/usr/bin/env bash
set -euo pipefail
# A hidden path must stay hidden through a same-UID process outside the sandbox.
# Probe permissions only; never read or modify ledger contents.
target=pip-ledger-alias-target.service
probe=pip-hermes-isolation.service
trap 'systemctl stop "$target" "$probe" || true' EXIT
systemd-run --quiet --unit="$target" --collect --uid=pip-control \
  --property=RuntimeMaxSec=120 /bin/sleep 110
target_pid=$(systemctl show "$target" -p MainPID --value)
test "$target_pid" -gt 0
sed -e "s|^ExecStart=.*|ExecStart=/bin/sh -c 'test ! -e /var/lib/pip/direct-queue \&\& test ! -e /var/lib/pip/artifacts \&\& test -w /var/lib/pip/hermes \&\& test -w /var/lib/pip/worktrees/hermes-scratch \&\& test ! -w /var/lib/pip/repositories \&\& test ! -r /var/lib/pip/ledger.db \&\& test ! -r /proc/$target_pid/root/var/lib/pip/ledger.db'|" \
  /work/repo/packaging/systemd/pip-hermes-gateway.service \
  > "/run/systemd/system/$probe"
install -d -m 0755 "/run/systemd/system/$probe.d"
printf '[Unit]\nConditionPathExists=\n[Service]\nType=oneshot\nRestart=no\nPrivateNetwork=yes\n' \
  > "/run/systemd/system/$probe.d/fixture.conf"
printf '[Service]\nPrivateUsers=no\n' > "/run/systemd/system/$probe.d/negative.conf"
systemctl daemon-reload
if systemctl start "$probe"; then
  echo 'unisolated same-UID ledger alias unexpectedly blocked' >&2
  exit 1
fi
test "$(systemctl show "$probe" -p ExecMainStatus --value)" = 1
unlink "/run/systemd/system/$probe.d/negative.conf"
systemctl daemon-reload
systemctl reset-failed "$probe"
systemctl start "$probe"
test "$(systemctl show "$probe" -p Result --value)" = success
test "$(systemctl show "$probe" -p PrivateUsers --value)" = yes
echo HERMES_SAME_UID_LEDGER_ALIAS_BLOCKED
