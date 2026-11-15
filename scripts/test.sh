#!/bin/sh
set -eu

TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test
TEST_DATABASE_URL_B=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test_b

./scripts/dev_db.sh reset-test
DATABASE_URL="$TEST_DATABASE_URL" cargo run --quiet --bin migrate
DATABASE_URL="$TEST_DATABASE_URL_B" cargo run --quiet --bin migrate
cargo test --all-targets
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --test postgres_migrations -- --ignored
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --lib db::url_evidence::tests::database_evidence_is_redacted_replaceable_and_backfillable -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --lib features::new_user_analysis::tests::url_shadow_observations_are_separate_and_private -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --lib features::new_user_analysis::tests::qr_only_evidence_refreshes_shadow_and_removed_links_clear_positive_state -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --features manual-moderation --test manual_moderation -- --ignored
TEST_DATABASE_URL="$TEST_DATABASE_URL" cargo test --test chat_db_mcp ask_mcp_client_starts_canonical_rmcp_child_with_env_clear_allowlist -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" TEST_DATABASE_URL_B="$TEST_DATABASE_URL_B" cargo test --test chat_db_mcp independent_community_databases_generate_manifests_and_start_isolated_mcp_servers -- --ignored --exact
