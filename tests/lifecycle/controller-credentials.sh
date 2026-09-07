#!/usr/bin/env bash
set -euo pipefail

names=(github.token github-reviewer-general.app github-reviewer-general.pem github-reviewer-secperf.app github-reviewer-secperf.pem)
sources=(pip-github-token pip-reviewer-general-app pip-reviewer-general-key pip-reviewer-secperf-app pip-reviewer-secperf-key)

# Verify in ExecStart, then replace this process with the actual controller.
# A separate post-start process is not the application's credential boundary.
if [[ ${1:-} == run ]]; then
  test "$(id -un)" = pip-control
  credential_directory=$3
  for name in "${names[@]}"; do
    if [[ $2 == all || ( $2 == token && $name == github.token ) ]]; then
      test "$(cat "$credential_directory/$name")" = "$name"
    else
      test ! -e "$credential_directory/$name"
    fi
  done
  echo CONTROLLER_CREDENTIAL_DELIVERY_OK
  shift 3
  exec "$@"
fi

# Disposable lifecycle container only; no account credentials or network calls.
test "$(id -u)" = 0
test -f /work/repo/Cargo.toml
test -d /run/systemd/system
unit=pip-controller@mdk.service
credential_root=$(mktemp -d /work/pip-controller-credentials.XXXXXX)
chmod 0700 "$credential_root"
install -d -m 0700 /etc/credstore
for index in "${!names[@]}"; do
  source_path="/etc/credstore/${sources[$index]}"
  test ! -e "$source_path"
  test ! -L "$source_path"
  printf '%s' "${names[$index]}" > "$credential_root/${names[$index]}"
  chmod 0600 "$credential_root/${names[$index]}"
done
dropin="/run/systemd/system/$unit.d/credential-fixture.conf"
install -d -m 0755 "${dropin%/*}"
controller_command=$(sed -n 's/^ExecStart=//p' /etc/systemd/system/pip-controller@.service)
test -n "$controller_command"
for phase in none token all; do
  if [[ $phase != none ]]; then
    for index in "${!names[@]}"; do
      [[ $phase == all || ${names[$index]} == github.token ]] || continue
      source_path="/etc/credstore/${sources[$index]}"
      if [[ ! -L $source_path ]]; then
        ln -s "$credential_root/${names[$index]}" "$source_path"
      fi
      if runuser -u pip-worker -- test -r "$source_path"; then
        echo 'worker can read a controller credential source' >&2
        exit 1
      fi
    done
  fi
  printf '[Service]\nPrivateNetwork=yes\nExecStart=\nExecStart=/bin/bash /source/tests/lifecycle/controller-credentials.sh run %s %%d %s\n' "$phase" "$controller_command" > "$dropin"
  systemctl daemon-reload
  systemctl start "$unit" || { journalctl -u "$unit" --no-pager; exit 1; }
  test "$(systemctl show "$unit" -p Result --value)" = success
done
unlink "$dropin"
systemctl daemon-reload
echo CONTROLLER_OPTIONAL_CREDENTIAL_STARTUP_OK
