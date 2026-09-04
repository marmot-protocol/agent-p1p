#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    printf '%s\n' "usage: $0 SIGNING_KEY_OUTPUT PUBLIC_KEY_OUTPUT" >&2
    exit 2
fi

signing_output=$1
public_output=$2
signing_key=${PIP_RELEASE_SIGNING_KEY_BASE64-}
public_key=${PIP_RELEASE_PUBLIC_KEY-}

validate_key() {
    name=$1
    value=$2
    if [ "${#value}" -ne 44 ]; then
        printf '%s\n' "$name must be one canonical base64-encoded 32-byte key" >&2
        exit 1
    fi
    case $value in
        *[!A-Za-z0-9+/=]*)
            printf '%s\n' "$name contains non-base64 characters" >&2
            exit 1
            ;;
    esac
    decoded_size=$(printf '%s' "$value" | openssl base64 -d -A | wc -c | tr -d ' ')
    if [ "$decoded_size" -ne 32 ]; then
        printf '%s\n' "$name must decode to exactly 32 bytes" >&2
        exit 1
    fi
}

validate_key PIP_RELEASE_SIGNING_KEY_BASE64 "$signing_key"
validate_key PIP_RELEASE_PUBLIC_KEY "$public_key"

for output in "$signing_output" "$public_output"; do
    if [ -e "$output" ] || [ -L "$output" ]; then
        printf '%s\n' "refusing to replace signing input: $output" >&2
        exit 1
    fi
done

umask 077
printf '%s\n' "$signing_key" >"$signing_output"
printf '%s\n' "$public_key" >"$public_output"
chmod 0600 "$signing_output" "$public_output"
