#!/bin/sh
# Offline contract for action commits whose action.yml was verified as node24.
set -eu
repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
references=$(awk '/uses:/ { for (i=1; i<=NF; i++) if ($i == "uses:") print $(i+1) }' "$repo_root"/.github/workflows/*.yml)
test -n "$references"
for reference in $references; do
    case "$reference" in
        actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09|\
        actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1|\
        actions/upload-artifact@b7c566a772e6b6bfb58ed0dc250532a479d7789f|\
        astral-sh/setup-uv@37802adc94f370d6bfd71619e3f0bf239e1f3b78)
            ;;
        *)
            printf 'Action runtime has not been verified as Node 24: %s\n' "$reference" >&2
            exit 1
            ;;
    esac
done
printf '%s\n' '{"ok":true,"result":"action_node24_pins_verified"}'
