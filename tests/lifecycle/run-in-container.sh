#!/usr/bin/env bash
set -euo pipefail

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

install -o root -g root -m 0400 /work/keys/public.key /etc/pip-v2-release-public.key

verified_values() {
  local release=$1
  "$release/root/bin/pip-control" verify-release \
    --release-root "$release/root" \
    --manifest "$release/release-manifest.json" \
    --signature "$release/release-manifest.sig" \
    --public-key /etc/pip-v2-release-public.key
}

install_version() {
  local release=$1
  local verified manifest_sha binary_sha
  verified=$(verified_values "$release")
  manifest_sha=$(jq -r .manifest_sha256 <<<"$verified")
  binary_sha=$(jq -r .binary_sha256 <<<"$verified")
  /work/repo/scripts/install-rust-control-plane.sh \
    --cohort "$release" \
    --public-key /etc/pip-v2-release-public.key \
    --manifest-sha256 "$manifest_sha" \
    --binary-sha256 "$binary_sha" >/dev/null
}

install_version /work/releases/v0
first_target=$(readlink -f /opt/pip-v2/current)
test -x /opt/pip-v2/current/bin/pip-control
test "$(stat -c '%U:%G:%a' /var/lib/pip-v2/ledger.db)" = pip-v2-control:pip-v2-control:600
grep -q '"enabled": false' /etc/pip-v2/repositories/mdk.json
grep -q '"dispatch_enabled": false' /etc/pip-v2/repositories/mdk.json
test "$(systemctl is-enabled pip-v2-shadow-reconcile.timer || true)" = disabled
test "$(systemctl is-active pip-v2-shadow-reconcile.timer || true)" = inactive
systemctl cat pip-v2-controller@.service >/dev/null
systemctl cat pip-v2-controller@.timer >/dev/null
test "$(systemctl is-enabled pip-v2-controller@mdk.timer || true)" = disabled
test "$(systemctl is-active pip-v2-controller@mdk.timer || true)" = inactive

install_version /work/releases/v0
test "$(readlink -f /opt/pip-v2/current)" = "$first_target"

install_version /work/releases/v1
second_target=$(readlink -f /opt/pip-v2/current)
test "$second_target" != "$first_target"
test -d "$first_target"
/opt/pip-v2/current/bin/pip-control status --database /var/lib/pip-v2/ledger.db --now 1787220000 \
  | jq -e '.ok and .ledger.schema_version == 5' >/dev/null

install -d -m 0755 /failure-bin
touch /run/pip-v2-fail-reload-once
cat >/failure-bin/systemctl <<'EOF'
#!/bin/sh
if [ "${1-}" = daemon-reload ] && [ -e /run/pip-v2-fail-reload-once ]; then
  rm -f /run/pip-v2-fail-reload-once
  exit 1
fi
exec /usr/bin/systemctl "$@"
EOF
chmod 0755 /failure-bin/systemctl
verified=$(verified_values /work/releases/v2)
if PATH=/failure-bin:/usr/bin:/bin /work/repo/scripts/install-rust-control-plane.sh \
  --cohort /work/releases/v2 \
  --public-key /etc/pip-v2-release-public.key \
  --manifest-sha256 "$(jq -r .manifest_sha256 <<<"$verified")" \
  --binary-sha256 "$(jq -r .binary_sha256 <<<"$verified")"; then
  echo "faulted upgrade unexpectedly succeeded" >&2
  exit 1
fi
test "$(readlink -f /opt/pip-v2/current)" = "$second_target"
test "$(systemctl is-enabled pip-v2-shadow-reconcile.timer || true)" = disabled
test "$(systemctl is-active pip-v2-shadow-reconcile.timer || true)" = inactive
test "$(find /opt/pip-v2/releases -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 2
systemctl cat pip-v2-shadow-reconcile.service >/dev/null
systemctl cat pip-v2-controller@.service >/dev/null
systemctl cat pip-v2-controller@.timer >/dev/null

printf '%s\n' "$first_target" "$second_target" >/work/expected-release-targets
