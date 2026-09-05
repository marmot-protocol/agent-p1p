#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --output PATH --version VERSION --built-at RFC3339 --builder-identity ID --signing-key PATH --public-key PATH" >&2
  exit 2
}

output=""
version=""
built_at=""
builder_identity=""
signing_key=""
public_key=""
while (($#)); do
  case "$1" in
    --output) output=${2-}; shift 2 ;;
    --version) version=${2-}; shift 2 ;;
    --built-at) built_at=${2-}; shift 2 ;;
    --builder-identity) builder_identity=${2-}; shift 2 ;;
    --signing-key) signing_key=${2-}; shift 2 ;;
    --public-key) public_key=${2-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$output" && -n "$version" && -n "$built_at" && -n "$builder_identity" && -n "$signing_key" && -n "$public_key" ]] || usage
[[ $(id -u) -ne 0 ]] || { echo "release builds must not run as root" >&2; exit 1; }
[[ ! -e "$output" ]] || { echo "release output already exists: $output" >&2; exit 1; }

repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$repo_root"
[[ -z $(git status --porcelain=v1 --untracked-files=all) ]] || {
  echo "release source tree must be clean" >&2
  exit 1
}
source_commit=$(git rev-parse --verify HEAD)
[[ $source_commit =~ ^[0-9a-f]{40}$ ]] || { echo "invalid source commit" >&2; exit 1; }
target=$(rustc -vV | sed -n 's/^host: //p')
rust_toolchain=$(rustc --version)
cargo_lock_sha256=$(shasum -a 256 Cargo.lock | awk '{print $1}')

output_parent=$(dirname -- "$output")
mkdir -p -- "$output_parent"
output_parent=$(cd -- "$output_parent" && pwd -P)
temporary=$(mktemp -d "$output_parent/.pip-release.XXXXXX")
cleanup() {
  if [[ -n ${temporary:-} && -d $temporary ]]; then
    chmod -R u+w -- "$temporary" 2>/dev/null || true
    rm -rf -- "$temporary"
  fi
}
trap cleanup EXIT

cargo build --release --locked -p pip-control
release_root="$temporary/root"
install -d -m 0755 \
  "$release_root/bin" \
  "$release_root/share/pip" \
  "$release_root/share/pip/docs" \
  "$release_root/share/pip/install"
install -m 0555 target/release/pip-control "$release_root/bin/pip-control"
install -m 0444 scripts/install-rust-control-plane.sh \
  "$release_root/share/pip/install/pip-install-release"
cp -R skills "$release_root/share/pip/skills"
cp -R config/target "$release_root/share/pip/config"
cp -R migration/target-v1 "$release_root/share/pip/contracts"
cp -R packaging/systemd "$release_root/share/pip/systemd"
cp skills/shared/workflow-contract/references/worker-result-contracts.md "$release_root/share/pip/docs/worker-result-contracts.md"
find "$release_root/share" -type d -exec chmod 0555 {} +
find "$release_root/share" -type f -exec chmod 0444 {} +
chmod 0555 "$release_root/bin"

"$release_root/bin/pip-control" seal-release \
  --release-root "$release_root" \
  --version "$version" \
  --source-commit "$source_commit" \
  --cargo-lock-sha256 "$cargo_lock_sha256" \
  --target "$target" \
  --rust-toolchain "$rust_toolchain" \
  --built-at "$built_at" \
  --builder-identity "$builder_identity" \
  --signing-key "$signing_key" \
  --expected-public-key "$public_key" \
  --manifest-output "$temporary/release-manifest.json" \
  --signature-output "$temporary/release-manifest.sig" >/dev/null

"$release_root/bin/pip-control" verify-release \
  --release-root "$release_root" \
  --manifest "$temporary/release-manifest.json" \
  --signature "$temporary/release-manifest.sig" \
  --public-key "$public_key" >/dev/null

chmod 0555 "$release_root"
chmod 0444 "$temporary/release-manifest.json" "$temporary/release-manifest.sig"
mv -- "$temporary" "$output"
temporary=""
echo "{\"ok\":true,\"source_commit\":\"$source_commit\",\"release\":\"$output\"}"
