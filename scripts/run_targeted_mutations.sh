#!/bin/sh
set -eu

if ! cargo mutants --version >/dev/null 2>&1; then
    echo "install cargo-mutants 27.1.0 before running targeted mutations" >&2
    exit 2
fi

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-0}"

run_mutations() {
    name=$1
    file=$2
    pattern=$3
    timeout=$4
    shift 4
    echo "mutation area: $name"
    status=0
    cargo mutants \
        "$@" \
        --file "$file" \
        --re "$pattern" \
        --baseline skip \
        --jobs 1 \
        --timeout "$timeout" \
        --cargo-test-arg=--lib || status=$?
    mkdir -p target/mutants
    rm -rf "target/mutants/$name"
    if [ -d mutants.out ]; then
        mv mutants.out "target/mutants/$name"
    else
        mkdir -p "target/mutants/$name"
        printf '%s\n' "cargo mutants exited $status without mutants.out" > "target/mutants/$name/runner-error.txt"
    fi
    if [ "$status" -ne 0 ]; then
        echo "mutation area $name failed; see target/mutants/$name" >&2
        exit "$status"
    fi
    if [ ! -s "target/mutants/$name/caught.txt" ]; then
        echo "mutation area $name caught no mutants; see target/mutants/$name" >&2
        exit 1
    fi
    echo "mutation area $name passed"
}

# Each range has a production counterexample oracle in the lib test suite.
run_mutations url-policy src/features/search/mcp.rs \
    'replace is_safe_fetch_url -> bool with (true|false)' \
    120
run_mutations mcp-scope src/features/chat_read_api/query.rs \
    'validate_scope_filters' \
    120
run_mutations memory-classification src/features/memory/service.rs \
    'classify_error' \
    120
run_mutations worker-supervision src/features/jobs/worker.rs \
    'WorkerSupervisor::record_exit' \
    120

# These emoji invariants only compile with the opt-in feature; keep their
# mutation run isolated from the default workspace feature set.
run_mutations emoji-unicode src/features/emoji_analysis.rs \
    'grapheme_indices|is_default_ignorable|canonical_emoji' \
    180 \
    --no-default-features \
    --features emoji

# Handler has no Default, so cargo-mutants cannot build a branch-order mutant.
# The explicit script swaps the two production branches and requires failure.
sh scripts/run_router_order_mutation.sh
