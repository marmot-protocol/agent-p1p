#!/bin/sh
set -eu

repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
workflow="$repo_root/.github/workflows/release.yml"
materializer="$repo_root/scripts/materialize-release-signing-inputs.sh"
release_builder="$repo_root/scripts/build-rust-release.sh"
repository_public_key="$repo_root/config/release-public.key"

fail() {
    printf '%s\n' "$1" >&2
    exit 1
}

grep -Fq 'name: Build Pip deployment' "$workflow" || fail "deployment workflow has the wrong operator-facing name"
grep -Eq '^  push:$' "$workflow" || fail "release builds must run automatically on master pushes"
grep -Fq 'branches: [master]' "$workflow" || fail "automatic release builds must target only master"
grep -Eq '^  workflow_dispatch:$' "$workflow" || fail "manual release rebuilds must remain available"
if grep -Eq '^  (pull_request|pull_request_target|workflow_run):' "$workflow"; then
    fail "signing workflow must not be triggered by untrusted PR or upstream-run inputs"
fi
if grep -Eq '^[[:space:]]+(source_commit|version):[[:space:]]*$' "$workflow"; then
    fail "deployment workflow still requires redundant operator inputs"
fi
grep -Fq 'group: pip-deployment' "$workflow" || fail "deployment workflow lacks global concurrency"
# The literal workflow shell command must not be expanded by this test.
# shellcheck disable=SC2016
grep -Fq 'test "$GITHUB_REF" = "refs/heads/master"' "$workflow" || fail "deployment workflow is not restricted to master"
# The literal workflow expression must not be shell-expanded by this test.
# shellcheck disable=SC2016
grep -Fq 'ref: ${{ github.sha }}' "$workflow" || fail "deployment checkout is not pinned to the triggering commit"
# The literal workflow shell command must not be expanded by this test.
# shellcheck disable=SC2016
grep -Fq 'release_version="git-${SOURCE_COMMIT:0:12}"' "$workflow" || fail "deployment identifier is not derived from the source commit"
# The obsolete workflow shell command must not be expanded by this test.
# shellcheck disable=SC2016
if grep -Fq 'built_at=$(git show -s --format=%cI "$SOURCE_COMMIT")' "$workflow"; then
    fail "deployment timestamp retains a non-canonical commit timezone"
fi
grep -Fq "date --utc" "$workflow" || fail "deployment timestamp is not normalized to UTC"
grep -Fq 'needs: [verify-rust, verify-systemd]' "$workflow" || fail "signing does not wait for every verification job"
test "$(grep -Fc 'environment: pip-release' "$workflow")" -eq 1 || fail "only the signing job may use the protected environment"
if grep -Fq 'secrets.PIP_RELEASE_PUBLIC_KEY' "$workflow"; then
    fail "public release key must not be stored as a secret"
fi
grep -Fq 'config/release-public.key' "$workflow" || fail "workflow does not use the repository trust anchor"
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
test -f "$repository_public_key" || fail "repository release public key is missing"
test "$(wc -l <"$repository_public_key" | tr -d ' ')" -eq 1 || fail "repository release public key must contain exactly one line"
test "$(tr -d '\n' <"$repository_public_key" | wc -c | tr -d ' ')" -eq 44 || fail "repository release public key is not canonical base64"
fixture=$(mktemp -d)
cleanup() {
    rm -rf -- "$fixture"
}
trap cleanup EXIT

signing_key='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA='
public_key=$(tr -d '\n' <"$repository_public_key")
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
