#!/bin/sh
set -eu

action_references=$(grep -R -h -E '^[[:space:]]*-[[:space:]]+uses:' .github/workflows 2>/dev/null || true)
if [ -n "$action_references" ]; then
    mutable_actions=$(printf '%s\n' "$action_references" | grep -E -v '@[0-9a-f]{40}([[:space:]]*(#.*)?)?$' || true)
    if [ -n "$mutable_actions" ]; then
        printf '%s\n' "GitHub Actions must use exact 40-character commit SHAs:" >&2
        printf '%s\n' "$mutable_actions" >&2
        exit 1
    fi
fi

dockerfiles=$(find . -type f \( -name 'Dockerfile' -o -name 'Dockerfile.*' \) -print)
for dockerfile in $dockerfiles; do
    mutable_images=$(grep -E '^[[:space:]]*FROM[[:space:]]+' "$dockerfile" \
        | grep -E -v '@sha256:[0-9a-f]{64}([[:space:]]|$)' || true)
    if [ -n "$mutable_images" ]; then
        printf '%s\n' "Container bases must use exact sha256 image-index digests: $dockerfile" >&2
        printf '%s\n' "$mutable_images" >&2
        exit 1
    fi
done

printf '%s\n' '{"ok":true,"result":"supply_chain_references_pinned"}'
