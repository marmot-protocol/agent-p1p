#!/usr/bin/env bash
set -euo pipefail
test "$(id -u)" = 0
test -d /run/systemd/system
# Expand PATH in the builder shell, not in the root caller.
# shellcheck disable=SC2016
runuser -u builder -- bash -c '
  cd /work/repo
  PATH=/usr/local/cargo/bin:$PATH cargo test --locked -p pip-control \
    --test builder_retry --no-run --message-format=json
' > /work/builder-retry-build.json
binary=$(jq -rs '[.[] | select(.profile.test == true and .target.name == "builder_retry") | .executable] | unique | if length == 1 then .[0] else error("ambiguous test binary") end' /work/builder-retry-build.json)
test -x "$binary"
"$binary" --ignored --exact root_cli_retry_checks_real_uid_stopped_units_and_empty_queue --nocapture
echo ROOT_BUILDER_RETRY_AUTHORIZATION_OK
