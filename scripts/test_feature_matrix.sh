#!/bin/sh
set -eu

feature_gate_test=config::tests::manual_moderation_requires_global_and_chat_gates_and_compiled_feature

cargo check --lib --locked
cargo check --no-default-features --lib --locked
if cargo tree --no-default-features --prefix none | rg -q '^emojis v'; then
    echo "emoji data dependency leaked into the no-default build" >&2
    exit 1
fi
cargo check --no-default-features --features emoji --lib --locked
python3 scripts/run_cargo_test_checked.py cargo test --no-default-features --features emoji --lib features::emoji_ --locked
python3 scripts/run_cargo_test_checked.py cargo test --no-default-features --lib telegram::commands::top_word_tests --locked
consumer_features=emoji,moderation,ask,manual-moderation,auto-comment,public-mcp
python3 scripts/run_cargo_test_checked.py cargo test --no-default-features --features "$consumer_features" --lib telegram:: --locked
python3 scripts/run_cargo_test_checked.py cargo test --no-default-features --features "$consumer_features" --lib features::qr_url::tests --locked
python3 scripts/run_cargo_test_checked.py cargo test --lib "$feature_gate_test" --locked -- --exact
python3 scripts/run_cargo_test_checked.py cargo test --no-default-features --lib "$feature_gate_test" --locked -- --exact
