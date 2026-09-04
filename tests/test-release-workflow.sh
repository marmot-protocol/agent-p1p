#!/bin/sh
set -eu

repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
workflow="$repo_root/.github/workflows/release.yml"
materializer="$repo_root/scripts/materialize-release-signing-inputs.sh"
release_builder="$repo_root/scripts/build-rust-release.sh"

fail() {
    printf '%s\n' "$1" >&2
    exit 1
}

grep -Fq 'source_commit:' "$workflow" || fail "release workflow lacks an exact source input"
grep -Fq 'group: pip-release' "$workflow" || fail "release workflow lacks global concurrency"
grep -Fq 'needs: [verify-rust, verify-systemd]' "$workflow" || fail "signing does not wait for every verification job"
test "$(grep -Fc 'environment: pip-release' "$workflow")" -eq 1 || fail "only the signing job may use the protected environment"
grep -Fq 'tests/test-release-workflow.sh' "$workflow" || fail "release verification omits its workflow contract"
grep -Fq 'scripts/test-systemd-lifecycle.sh' "$workflow" || fail "release verification omits the lifecycle gate"
grep -Fq 'root/share/pip/install/pip-install-release' "$workflow" || fail "workflow does not export the signed installer"
grep -Fq 'share/pip/install/pip-install-release' "$release_builder" || fail "installer is outside the signed release manifest"
# The literal workflow expression must not be shell-expanded by this test.
# shellcheck disable=SC2016
if grep -Fq 'base64 --decode > "$RUNNER_TEMP/release-signing.key"' "$workflow"; then
    fail "release workflow decodes the base64 seed into an invalid raw key file"
fi

test -x "$materializer" || fail "release signing-input materializer is missing"
fixture=$(mktemp -d)
cleanup() {
    rm -rf -- "$fixture"
}
trap cleanup EXIT

signing_key='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA='
public_key='AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE='
PIP_RELEASE_SIGNING_KEY_BASE64="$signing_key" \
PIP_RELEASE_PUBLIC_KEY="$public_key" \
    "$materializer" "$fixture/signing.key" "$fixture/public.key"

test "$(tr -d '\n' <"$fixture/signing.key")" = "$signing_key" || fail "signing key file changed representation"
test "$(tr -d '\n' <"$fixture/public.key")" = "$public_key" || fail "public key file changed representation"

case $(uname -s) in
    Darwin)
        signing_mode=$(stat -f '%Lp' "$fixture/signing.key")
        public_mode=$(stat -f '%Lp' "$fixture/public.key")
        ;;
    *)
        signing_mode=$(stat -c '%a' "$fixture/signing.key")
        public_mode=$(stat -c '%a' "$fixture/public.key")
        ;;
esac
test "$signing_mode" = 600 || fail "signing key mode is not 0600"
test "$public_mode" = 600 || fail "public key mode is not 0600"

if PIP_RELEASE_SIGNING_KEY_BASE64='not-base64' \
   PIP_RELEASE_PUBLIC_KEY="$public_key" \
   "$materializer" "$fixture/bad-signing.key" "$fixture/bad-public.key" >/dev/null 2>&1; then
    fail "invalid signing key unexpectedly materialized"
fi

printf '%s\n' '{"ok":true,"result":"release_workflow_contract_passed"}'
