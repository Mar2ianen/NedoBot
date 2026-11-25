#!/usr/bin/env bash
set -euo pipefail

environment_file="${1:?usage: set_privacy_mode_observe.sh <environment-file>}"
test -f "$environment_file"
directory="$(dirname -- "$environment_file")"
temporary_file="$(mktemp "$directory/.privacy-mode.XXXXXXXX")"
cleanup() {
    rm -f -- "$temporary_file"
}
trap cleanup EXIT

awk '
    /^[[:space:]]*(export[[:space:]]+)?NEDOBOT_PRIVACY_MODE_POLICY[[:space:]]*=/ { next }
    { print }
' "$environment_file" > "$temporary_file"
printf '%s\n' 'NEDOBOT_PRIVACY_MODE_POLICY=observe' >> "$temporary_file"
chmod --reference="$environment_file" "$temporary_file"
chown --reference="$environment_file" "$temporary_file"
mv -f -- "$temporary_file" "$environment_file"
trap - EXIT

test "$(grep -Ec '^[[:space:]]*(export[[:space:]]+)?NEDOBOT_PRIVACY_MODE_POLICY[[:space:]]*=' "$environment_file")" -eq 1
