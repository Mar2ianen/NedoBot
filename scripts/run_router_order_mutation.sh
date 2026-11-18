#!/bin/sh
set -eu

# cargo-mutants can only replace the whole command_first() return value here,
# and Handler has no Default implementation, so that mutant is unviable. Apply
# the smallest semantic mutation directly: swap the command and message
# branches. The production router characterization test must fail on it.
file=src/telegram/router/commands.rs
backup=target/router-commands.rs.bak

if ! git diff --quiet -- "$file" || ! git diff --cached --quiet -- "$file"; then
    echo "refusing to mutate a dirty $file" >&2
    exit 2
fi

mkdir -p target
cp "$file" "$backup"
restore() {
    cp "$backup" "$file"
    rm -f "$backup"
}
trap restore EXIT INT TERM

python3 - "$file" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
source = path.read_text(encoding="utf-8")
command = ".branch(command_handler)"
message = ".branch(message_handler)"
if source.count(command) != 1 or source.count(message) != 1:
    raise SystemExit("router branch markers are not unique")
swapped = (
    source.replace(command, ".branch(__ROUTER_MUTANT__)")
    .replace(message, command)
    .replace(".branch(__ROUTER_MUTANT__)", message)
)
if swapped == source:
    raise SystemExit("router branch swap did not change the file")
path.write_text(swapped, encoding="utf-8")
PY

log=target/router-order-mutation.log
if cargo test --lib \
    telegram::router::tests::actual_router_builder_prefers_command_and_falls_back_for_unknown_command \
    --locked -- --exact >"$log" 2>&1; then
    echo "error: router branch-order mutation survived the production router test" >&2
    tail -20 "$log" >&2
    exit 1
fi

if ! grep -q 'test result: FAILED' "$log"; then
    echo "error: router mutation did not reach a failing assertion" >&2
    tail -30 "$log" >&2
    exit 2
fi

echo "router branch-order mutation was killed by the production router test"
