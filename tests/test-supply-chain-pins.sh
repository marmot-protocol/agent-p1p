#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
fixture=$(mktemp -d)
cleanup() {
  rm -rf -- "$fixture"
}
trap cleanup EXIT

mkdir -p "$fixture/.github/workflows" "$fixture/container"
cat >"$fixture/.github/workflows/ci.yml" <<'EOF'
jobs:
  test:
    steps:
      - uses: actions/checkout@v4
EOF
cat >"$fixture/container/Dockerfile" <<'EOF'
FROM rust:latest
EOF
if (cd "$fixture" && "$repo_root/scripts/check-supply-chain-pins.sh") >/dev/null 2>&1; then
  echo "mutable supply-chain references unexpectedly passed" >&2
  exit 1
fi

cat >"$fixture/.github/workflows/ci.yml" <<'EOF'
jobs:
  test:
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
EOF
cat >"$fixture/container/Dockerfile" <<'EOF'
FROM rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663
EOF
(cd "$fixture" && "$repo_root/scripts/check-supply-chain-pins.sh") >/dev/null
printf '%s\n' '{"ok":true,"result":"supply_chain_pin_regression_passed"}'
