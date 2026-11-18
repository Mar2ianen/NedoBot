#!/bin/sh
set -eu

if ! cargo mutants --version >/dev/null 2>&1; then
    echo "install cargo-mutants 27.1.0 before running targeted mutations" >&2
    exit 2
fi

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-0}"

exec cargo mutants \
    --file src/features/search/mcp.rs \
    --re 'replace is_safe_fetch_url -> bool with (true|false)' \
    --baseline skip \
    --jobs 1 \
    --timeout 120 \
    --cargo-test-arg=--lib
