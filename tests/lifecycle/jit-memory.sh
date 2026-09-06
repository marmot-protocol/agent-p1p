#!/usr/bin/env bash
set -euo pipefail
# Offline syscall-level regression for the V8 startup boundary.
cc -Wall -Wextra -Werror /source/tests/lifecycle/jit-memory.c -o /work/jit-memory
chmod 0755 /work/jit-memory
unit=pip-jit-memory-fixture.service
sed -e 's|^ExecStart=.*|ExecStart=/work/jit-memory|' \
  /work/repo/packaging/systemd/pip-direct-worker@.service > "/run/systemd/system/$unit"
install -d -m 0755 "/run/systemd/system/$unit.d"
printf '[Service]\nMemoryDenyWriteExecute=yes\nPrivateNetwork=yes\n' > "/run/systemd/system/$unit.d/negative.conf"
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
echo DIRECT_WORKER_JIT_MEMORY_BOUNDARY_OK
