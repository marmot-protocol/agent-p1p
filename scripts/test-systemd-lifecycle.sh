#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
image="pip-v2-systemd-lifecycle:local"
container="pip-v2-systemd-lifecycle-$$"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker build --file "$repo_root/tests/lifecycle/Dockerfile" --tag "$image" "$repo_root/tests/lifecycle"
docker run --detach --name "$container" --privileged --cgroupns=private \
  --tmpfs /run --tmpfs /run/lock \
  --volume "$repo_root:/source:ro" \
  "$image" >/dev/null

for _ in $(seq 1 60); do
  state=$(docker exec "$container" systemctl is-system-running 2>/dev/null || true)
  if [[ $state == running || $state == degraded ]]; then
    break
  fi
  sleep 1
done
[[ ${state:-} == running || ${state:-} == degraded ]] || {
  docker logs "$container" >&2
  echo "disposable systemd did not become ready" >&2
  exit 1
}

docker exec "$container" /source/tests/lifecycle/run-in-container.sh
docker restart "$container" >/dev/null
for _ in $(seq 1 60); do
  state=$(docker exec "$container" systemctl is-system-running 2>/dev/null || true)
  if [[ $state == running || $state == degraded ]]; then
    break
  fi
  sleep 1
done
[[ ${state:-} == running || ${state:-} == degraded ]]
docker exec "$container" bash -lc '
  set -euo pipefail
  mapfile -t expected </work/expected-release-targets
  test "$(readlink -f /opt/pip-v2/current)" = "${expected[1]}"
  test -d "${expected[0]}"
  test "$(systemctl is-enabled pip-v2-shadow-reconcile.timer || true)" = disabled
  test "$(systemctl is-active pip-v2-shadow-reconcile.timer || true)" = inactive
  test "$(systemctl is-enabled pip-v2-controller@mdk.timer || true)" = disabled
  test "$(systemctl is-active pip-v2-controller@mdk.timer || true)" = inactive
  /opt/pip-v2/current/bin/pip-control status --database /var/lib/pip-v2/ledger.db --now 1787220001 \
    | jq -e ".ok and .ledger.schema_version == 3" >/dev/null
'

echo '{"ok":true,"clean_install":true,"reinstall":true,"upgrade":true,"rollback":true,"restart_recovery":true,"timer_enabled":false}'
