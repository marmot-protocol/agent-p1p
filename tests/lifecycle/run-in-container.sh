#!/usr/bin/env bash
set -euo pipefail

bash /source/tests/lifecycle/timer-restart.sh /source/packaging/systemd

rm -rf /work/repo
install -d -m 0755 /work
cp -a /source /work/repo
if [[ -n $(git -C /work/repo status --porcelain=v1 --untracked-files=all) ]]; then
  git -C /work/repo add -A
  git -C /work/repo -c user.name=lifecycle -c user.email=lifecycle.invalid commit -m lifecycle-candidate >/dev/null
fi
useradd --create-home --shell /bin/bash builder
chown -R builder:builder /work/repo /work

runuser -u builder -- bash -lc '
  set -euo pipefail
  export PATH=/usr/local/cargo/bin:$PATH
  cd /work/repo
  install -d -m 0700 /work/keys /work/releases
  printf "ERERERERERERERERERERERERERERERERERERERERERE=" >/work/keys/signing.key
  chmod 0600 /work/keys/signing.key
  cargo build --locked -p pip-control
  target/debug/pip-control derive-public-key --signing-key /work/keys/signing.key \
    | jq -r .public_key >/work/keys/public.key
  for version in 0.1.0 0.1.1 0.1.2; do
    suffix=${version##*.}
    scripts/build-rust-release.sh \
      --output "/work/releases/v$suffix" \
      --version "$version" \
      --built-at "2026-08-20T12:00:0${suffix}Z" \
      --builder-identity disposable-systemd \
      --signing-key /work/keys/signing.key \
      --public-key /work/keys/public.key >/dev/null
  done
'

install -o root -g root -m 0400 /work/keys/public.key /etc/pip-release-public.key

verified_values() {
  local release=$1
  "$release/root/bin/pip-control" verify-release \
    --release-root "$release/root" \
    --manifest "$release/release-manifest.json" \
    --signature "$release/release-manifest.sig" \
    --public-key /etc/pip-release-public.key
}

install_version() (
  local release=$1
  # Cover both the restrictive operator wrapper and an overly permissive caller.
  umask "${2:-077}"
  local verified manifest_sha binary_sha
  verified=$(verified_values "$release")
  manifest_sha=$(jq -r .manifest_sha256 <<<"$verified")
  binary_sha=$(jq -r .binary_sha256 <<<"$verified")
  /work/repo/scripts/install-rust-control-plane.sh \
    --cohort "$release" \
    --public-key /etc/pip-release-public.key \
    --manifest-sha256 "$manifest_sha" \
    --binary-sha256 "$binary_sha" >/dev/null
)

assert_service_release_access() {
  local release=$1
  local installed_root
  installed_root=$(readlink -f /opt/pip/current)
  test -z "$(find /opt/pip/releases -type d ! -perm 0755 -print -quit)"
  # Share only public verification inputs, never ledger or signing-key copies.
  install -d -m 0755 /run/pip-release-access-probe
  install -m 0444 "$release/release-manifest.json" "$release/release-manifest.sig" \
    /run/pip-release-access-probe/
  install -m 0444 /etc/pip-release-public.key /run/pip-release-access-probe/public.key
  for identity in pip-control pip-worker pip-ingress; do
    # Exercise exec as the real service UID, not just root's access checks.
    systemd-run --quiet --wait --pipe --collect --property="User=$identity" \
      /opt/pip/current/bin/pip-control verify-release \
      --release-root "$installed_root" \
      --manifest /run/pip-release-access-probe/release-manifest.json \
      --signature /run/pip-release-access-probe/release-manifest.sig \
      --public-key /run/pip-release-access-probe/public.key \
      | jq -e '.ok' >/dev/null
    runuser -u "$identity" -- test -r /opt/pip/current/SOURCE.COMMIT
    runuser -u "$identity" -- test -r /etc/pip/repositories/mdk.json
    runuser -u "$identity" -- test -r /opt/pip/current/share/pip/skills/planner/SKILL.md
  done
  # Release traversal must not broaden access to mutable control-plane state.
  if runuser -u pip-worker -- test -r /var/lib/pip/ledger.db \
    || runuser -u pip-ingress -- test -r /var/lib/pip/ledger.db; then
    echo 'release installation exposed the private ledger' >&2
    exit 1
  fi
}

# Mirror Pirate's pre-install storage preparation: the operator creates the
# mount point before the Pip service identities exist, so these two bootstrap
# directories are necessarily root-owned. The installer must adopt only this
# exact, empty, safe shape after it creates the identities.
install -d -o root -g root -m 0755 /var/lib/pip
install -d -o root -g root -m 0770 /work/bootstrap-worktrees
mount -t tmpfs -o size=16m pip-worktree-test /work/bootstrap-worktrees
chmod 0770 /work/bootstrap-worktrees
install -d -o root -g root -m 0770 /var/lib/pip/worktrees
mount --bind /work/bootstrap-worktrees /var/lib/pip/worktrees
test "$(stat -c '%U:%G:%a' /var/lib/pip)" = root:root:755
test "$(stat -c '%U:%G:%a' /var/lib/pip/worktrees)" = root:root:770

install_version /work/releases/v0
assert_service_release_access /work/releases/v0
first_target=$(readlink -f /opt/pip/current)
bash /source/tests/lifecycle/workspace-handoff.sh
bash /source/tests/lifecycle/jit-memory.sh
bash /source/tests/lifecycle/hermes-isolation.sh
bash /source/tests/lifecycle/builder-retry.sh
test -x /opt/pip/current/bin/pip-control
test "$(stat -c '%U:%G:%a' /var/lib/pip/ledger.db)" = pip-control:pip-control:600
test "$(stat -c '%U:%G:%a' /var/lib/pip)" = pip-control:pip-control:710
test "$(stat -c '%U:%G:%a' /var/lib/pip/worktrees)" = pip-control:pip-control:770
test "$(stat -c '%U:%G:%a' /var/lib/pip/worktrees/hermes-scratch)" = pip-control:pip-control:700
test "$(getent passwd pip-ingress | cut -d: -f6-7)" = /nonexistent:/usr/sbin/nologin
test "$(id -Gn pip-ingress)" = pip-ingress
test "$(stat -c '%U:%G:%a' /var/spool/pip-webhooks)" = root:root:711
test "$(stat -c '%U:%G:%a' /var/spool/pip-webhooks/receipts)" = pip-ingress:pip-control:2750
test "$(stat -c '%U:%G:%a' /var/spool/pip-webhooks/pending)" = pip-ingress:pip-control:2770
test "$(stat -c '%U:%G:%a' /var/spool/pip-webhooks/processed)" = pip-control:pip-control:711
grep -q '"enabled": false' /etc/pip/repositories/mdk.json
grep -q '"dispatch_enabled": false' /etc/pip/repositories/mdk.json
test "$(systemctl is-enabled pip-shadow-reconcile.timer || true)" = disabled
test "$(systemctl is-active pip-shadow-reconcile.timer || true)" = inactive
systemctl cat pip-controller@.service >/dev/null
systemctl cat pip-controller@.timer >/dev/null
systemctl cat pip-webhook-ingress.service >/dev/null
systemctl cat pip-webhook-consumer@.service >/dev/null
systemctl cat pip-webhook-consumer@.timer >/dev/null
systemd-analyze verify \
  /etc/systemd/system/pip-webhook-ingress.service \
  /etc/systemd/system/pip-webhook-consumer@.service \
  /etc/systemd/system/pip-webhook-consumer@.timer
test "$(systemctl is-enabled pip-controller@mdk.timer || true)" = disabled
test "$(systemctl is-active pip-controller@mdk.timer || true)" = inactive
test "$(systemctl is-enabled pip-webhook-ingress.service || true)" = disabled
test "$(systemctl is-active pip-webhook-ingress.service || true)" = inactive
test "$(systemctl is-enabled pip-webhook-consumer@mdk.timer || true)" = disabled
test "$(systemctl is-active pip-webhook-consumer@mdk.timer || true)" = inactive

# A deployed operator policy is state, not a release artifact. Keep every
# execution timer disabled while proving active settings survive upgrades.
jq '.revision += 1000 | .intake.enabled = true | .intake.paused = false |
    .dispatch_enabled = true | .github.automation_actor_id = 1 |
    .github.reviewer_general_actor_id = 2 | .github.reviewer_secperf_actor_id = 3' \
  /etc/pip/repositories/mdk.json > /work/operator-policy.json
install -o root -g root -m 0444 /work/operator-policy.json /etc/pip/repositories/mdk.json
sha256sum /etc/pip/repositories/mdk.json > /work/operator-policy.sha256

install_version /work/releases/v0 000
assert_service_release_access /work/releases/v0
test "$(readlink -f /opt/pip/current)" = "$first_target"
sha256sum --check /work/operator-policy.sha256

install_version /work/releases/v1
assert_service_release_access /work/releases/v1
sha256sum --check /work/operator-policy.sha256
second_target=$(readlink -f /opt/pip/current)
test "$second_target" != "$first_target"
test -d "$first_target"
/opt/pip/current/bin/pip-control status --database /var/lib/pip/ledger.db --now 1787220000 \
  | jq -e '.ok and .ledger.schema_version == 10' >/dev/null

install -d -m 0755 /failure-bin
touch /run/pip-fail-reload-once
cat >/failure-bin/systemctl <<'EOF'
#!/bin/sh
if [ "${1-}" = daemon-reload ] && [ -e /run/pip-fail-reload-once ]; then
  rm -f /run/pip-fail-reload-once
  exit 1
fi
exec /usr/bin/systemctl "$@"
EOF
chmod 0755 /failure-bin/systemctl
verified=$(verified_values /work/releases/v2)
if PATH=/failure-bin:/usr/bin:/bin /work/repo/scripts/install-rust-control-plane.sh \
  --cohort /work/releases/v2 \
  --public-key /etc/pip-release-public.key \
  --manifest-sha256 "$(jq -r .manifest_sha256 <<<"$verified")" \
  --binary-sha256 "$(jq -r .binary_sha256 <<<"$verified")"; then
  echo "faulted upgrade unexpectedly succeeded" >&2
  exit 1
fi
test "$(readlink -f /opt/pip/current)" = "$second_target"
sha256sum --check /work/operator-policy.sha256
test "$(systemctl is-enabled pip-shadow-reconcile.timer || true)" = disabled
test "$(systemctl is-active pip-shadow-reconcile.timer || true)" = inactive
test "$(find /opt/pip/releases -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 2
systemctl cat pip-shadow-reconcile.service >/dev/null
systemctl cat pip-controller@.service >/dev/null
systemctl cat pip-controller@.timer >/dev/null
systemctl cat pip-webhook-ingress.service >/dev/null
systemctl cat pip-webhook-consumer@.service >/dev/null
systemctl cat pip-webhook-consumer@.timer >/dev/null

printf '%s\n' "$first_target" "$second_target" >/work/expected-release-targets
