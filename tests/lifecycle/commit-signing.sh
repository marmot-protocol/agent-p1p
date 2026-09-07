#!/usr/bin/env bash
set -euo pipefail

# Disposable container only. These are generated fixture keys, never GitHub keys.
test "$(id -u)" = 0
test -f /work/repo/Cargo.toml
test -d /run/systemd/system
runuser -u builder -- bash -c '
  cd /work/repo
  PATH=/usr/local/cargo/bin:$PATH cargo test --locked -p pip-control --lib --no-run --message-format=json
' > /work/signing-fixture-build.json
binary=$(jq -rs '[.[] | select(.profile.test == true and .target.name == "pip_control") | .executable] | unique | if length == 1 then .[0] else error("ambiguous test binary") end' /work/signing-fixture-build.json)
test -x "$binary"
credentials=$(mktemp -d /work/pip-signing-credentials.XXXXXX)
fixture=$(mktemp -d /work/pip-signing-workspace.XXXXXX)
chown pip-control:pip-control "$fixture"
chmod 0710 "$fixture"
ssh-keygen -q -t ed25519 -N '' -f "$credentials/key"
jq -n --arg public_key "$(<"$credentials/key.pub")" \
  '{schema_version:1,actor_id:42,name:"Fixture Bot",email:"42+fixture@users.noreply.github.com",public_key:$public_key}' \
  > "$credentials/identity.json"
chmod 0600 "$credentials/identity.json"
if runuser -u pip-worker -- test -r "$credentials/key"; then
  echo 'worker can read controller signing key' >&2
  exit 1
fi
unit=pip-signing-credential-fixture.service
sed \
  -e "s|^ExecStart=.*|ExecStart=$binary --ignored --exact commit_signing::tests::service_signing_credential_boundary --nocapture|" \
  -e "s|^WorkingDirectory=.*|WorkingDirectory=$fixture|" \
  -e '/^Environment=/d' \
  -e '/^LoadCredential=/d' \
  -e "s|^ReadWritePaths=|ReadWritePaths=$fixture |" \
  /work/repo/packaging/systemd/pip-controller@.service > "/run/systemd/system/$unit"
install -d -m 0755 "/run/systemd/system/$unit.d"
printf '[Service]\nEnvironment=PIP_SIGNING_FIXTURE_ROOT=%s\nPrivateNetwork=yes\nLoadCredential=pip-signing-fixture-key:%s/key\nLoadCredential=pip-signing-fixture-identity:%s/identity.json\n' \
  "$fixture" "$credentials" "$credentials" > "/run/systemd/system/$unit.d/fixture.conf"
systemctl daemon-reload
systemctl start "$unit" || { journalctl -u "$unit" --no-pager; exit 1; }
test "$(systemctl show "$unit" -p Result --value)" = success
journalctl -u "$unit" --no-pager | grep -F CONTROLLER_SIGNING_CREDENTIAL_SANDBOX_OK
