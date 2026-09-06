#!/usr/bin/env bash
set -euo pipefail

# Run only in the disposable lifecycle container, after installer-created UIDs.
test "$(id -u)" = 0
test -f /work/repo/Cargo.toml
test -d /run/systemd/system
test "$(id -u pip-control)" != "$(id -u pip-worker)"
runuser -u builder -- bash -c '
  cd /work/repo
  PATH=/usr/local/cargo/bin:$PATH cargo test --locked -p pip-executor \
    --test isolated_workspace --no-run --message-format=json
' > /work/workspace-handoff-build.json
binary=$(jq -rs '[.[] | select(.profile.test == true and .target.name == "isolated_workspace") | .executable] | unique | if length == 1 then .[0] else error("ambiguous test binary") end' /work/workspace-handoff-build.json)
test -x "$binary"
fixture=$(mktemp -d /work/pip-workspace-handoff.XXXXXX)
chown pip-control:pip-control "$fixture"
chmod 0710 "$fixture"
runuser -u pip-control -- env PIP_HANDOFF_ROOT="$fixture" PIP_HANDOFF_PHASE=prepare \
  /bin/sh -c 'umask 0077; exec "$@"' pip-handoff \
  "$binary" --ignored --exact service_identity_workspace_handoff --nocapture

# Exercise the installed worker's actual restrictions, not just a shell with
# a different UID. Replace only the command and fixture-specific paths.
unit=pip-workspace-handoff-fixture.service
sed \
  -e 's|^Description=.*|Description=Offline Pip workspace handoff test|' \
  -e "s|^ExecStart=.*|ExecStart=$binary --ignored --exact service_identity_workspace_handoff --nocapture|" \
  -e "s|^WorkingDirectory=.*|WorkingDirectory=$fixture/workspaces/repo-123-issue-456-workflow-1|" \
  -e '/^Environment=/d' \
  -e "s|^ReadWritePaths=|ReadWritePaths=$fixture/workspaces |" \
  -e "s|^InaccessiblePaths=|InaccessiblePaths=$fixture/private-repository |" \
  /work/repo/packaging/systemd/pip-direct-worker@.service > "/run/systemd/system/$unit"
install -d -m 0755 "/run/systemd/system/$unit.d"
printf '[Service]\nEnvironment=PIP_HANDOFF_ROOT=%s PIP_HANDOFF_PHASE=worker\nPrivateNetwork=yes\n' "$fixture" \
  > "/run/systemd/system/$unit.d/fixture.conf"
systemctl daemon-reload
# Prove the fixture catches the original startup failure before testing the
# repaired layout. This touches only this disposable fixture's case root.
chmod 0700 "$fixture/workspaces/repo-123-issue-456-workflow-1"
if systemctl start "$unit"; then
  echo "worker unexpectedly entered a controller-private workspace" >&2
  exit 1
fi
test "$(systemctl show "$unit" -p ExecMainStatus --value)" = 200
chmod 2770 "$fixture/workspaces/repo-123-issue-456-workflow-1"
systemctl reset-failed "$unit"
systemctl start "$unit" || { journalctl -u "$unit" --no-pager; exit 1; }
test "$(systemctl show "$unit" -p Result --value)" = success
runuser -u pip-control -- env PIP_HANDOFF_ROOT="$fixture" PIP_HANDOFF_PHASE=reconcile \
  /bin/sh -c 'umask 0077; exec "$@"' pip-handoff \
  "$binary" --ignored --exact service_identity_workspace_handoff --nocapture
echo WORKSPACE_TWO_UID_SANDBOX_HANDOFF_OK
