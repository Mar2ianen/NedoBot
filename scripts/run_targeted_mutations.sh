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
    echo "mutation area: $name"
    status=0
    cargo mutants \
        --file "$file" \
        --re "$pattern" \
        --baseline skip \
        --jobs 1 \
        --timeout 120 \
        --cargo-test-arg=--lib || status=$?
    mkdir -p target/mutants
    rm -rf "target/mutants/$name"
    mv mutants.out "target/mutants/$name"
    if [ "$status" -ne 0 ]; then
        echo "mutation area $name failed; see target/mutants/$name" >&2
        exit "$status"
    fi
    echo "mutation area $name passed"
}

# Each range has a production counterexample oracle in the lib test suite.
run_mutations url-policy src/features/search/mcp.rs \
    'replace is_safe_fetch_url -> bool with (true|false)'
run_mutations mcp-scope src/features/chat_read_api/query.rs \
    'validate_scope_filters'
run_mutations memory-classification src/features/memory/service.rs \
    'classify_error'
run_mutations worker-supervision src/features/jobs/worker.rs \
    'WorkerSupervisor::record_exit'

# Handler has no Default, so cargo-mutants cannot build a branch-order mutant.
# The explicit script swaps the two production branches and requires failure.
sh scripts/run_router_order_mutation.sh
