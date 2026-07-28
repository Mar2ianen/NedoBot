#!/bin/sh
set -eu

TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test

./scripts/dev_db.sh reset-test
DATABASE_URL="$TEST_DATABASE_URL" cargo run --quiet --bin migrate
cargo test --all-targets
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --test postgres_migrations -- --ignored
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --test chat_db_mcp -- --ignored
