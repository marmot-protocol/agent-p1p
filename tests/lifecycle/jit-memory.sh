#!/usr/bin/env bash
set -euo pipefail
# Offline syscall-level regression for the V8 startup boundary.
cc -Wall -Wextra -Werror /source/tests/lifecycle/jit-memory.c -o /work/jit-memory
chmod 0755 /work/jit-memory
for template in pip-direct-worker@.service pip-hermes-gateway.service; do
  unit="pip-jit-${template/@/fixture}"
  sed -e 's|^ExecStart=.*|ExecStart=/work/jit-memory|' \
    "/work/repo/packaging/systemd/$template" > "/run/systemd/system/$unit"
  install -d -m 0755 "/run/systemd/system/$unit.d"
  # This syscall fixture does not bootstrap Hermes. Clear only its ownership
  # marker condition so systemd cannot report a skipped probe as successful.
  printf '[Unit]\nConditionPathExists=\n[Service]\nType=oneshot\nRestart=no\nPrivateNetwork=yes\n' > "/run/systemd/system/$unit.d/fixture.conf"
  printf '[Service]\nMemoryDenyWriteExecute=yes\n' > "/run/systemd/system/$unit.d/negative.conf"
  systemctl daemon-reload
  if systemctl start "$unit"; then
    echo 'negative JIT-memory fixture unexpectedly succeeded' >&2
    exit 1
  fi
  test "$(systemctl show "$unit" -p ExecMainStatus --value)" = 77
  unlink "/run/systemd/system/$unit.d/negative.conf"
  systemctl daemon-reload
  systemctl reset-failed "$unit"
  systemctl start "$unit"
  test "$(systemctl show "$unit" -p Result --value)" = success
  test "$(systemctl show "$unit" -p NoNewPrivileges --value)" = yes
  test "$(systemctl show "$unit" -p ProtectSystem --value)" = strict
  echo "EXECUTION_JIT_MEMORY_BOUNDARY_OK $template"
done
