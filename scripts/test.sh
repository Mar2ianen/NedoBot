#!/bin/sh
set -eu

# The single PostgreSQL lifecycle test retains a large async future on its test
# thread. 8 MiB passes locally and in CI; 2 MiB overflows, so keep a bounded 4x
# margin without reserving the old 32 MiB per test thread.
export RUST_MIN_STACK=${RUST_MIN_STACK:-8388608}
# Limit simultaneous full-feature binary links on developer workstations.
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}

TEST_DATABASE_URL=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test
TEST_DATABASE_URL_B=postgres://tg_ai_bot_dev:tg_ai_bot_dev@127.0.0.1:5433/tg_ai_bot_test_b

./scripts/dev_db.sh reset-test
DATABASE_URL="$TEST_DATABASE_URL" cargo run --quiet --locked --bin migrate
DATABASE_URL="$TEST_DATABASE_URL_B" cargo run --quiet --locked --bin migrate
python3 scripts/run_cargo_test_checked.py cargo test --workspace --all-targets --locked
run_test() {
    python3 scripts/run_cargo_test_checked.py cargo test --locked "$@"
}
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --test postgres_migrations -- --ignored
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --test adversarial_harness post_comment_sql_matches_reference_lease_and_delivery_trace -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --test adversarial_harness generated_post_comment_claims_reject_stale_and_duplicate_delivery -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::user_profiles::enrichment::tests::saturated_and_closed_queue_preserves_ingested_audit_intents -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::user_profiles::enrichment::tests::two_chats_share_profile_fetch_but_keep_both_audits -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::user_profiles::enrichment::tests::fast_audit_ack_fences_revisions_and_reclaims -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::qr_url::tests::qr_worker_persists_fresh_alias_relation_and_rejects_superseded_media -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib telegram::manual_moderation::tests::username_sanctions_require_live_ownership_and_resume_preserves_provenance -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::voice::pipeline::tests::voice_partial_delivery_is_persisted_and_never_replayed_after_restart -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib db::url_evidence::tests::database_evidence_is_redacted_replaceable_and_backfillable -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::new_user_analysis::tests::url_shadow_observations_are_separate_and_private -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::new_user_analysis::tests::url_shadow_lock_serializes_same_user_without_blocking_other_users -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::new_user_analysis::tests::cancelled_url_shadow_refresh_releases_its_session_lock -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::new_user_analysis::tests::qr_only_evidence_refreshes_shadow_and_removed_links_clear_positive_state -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --bin export_url_inventory tests::export_separates_current_flags_from_latest_operator_labels -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --bin sync_lols_banlist tests::sync_is_incremental_idempotent_and_guarded -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --bin sync_lols_banlist tests::advisory_lock_blocks_a_second_sync_session -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --lib features::url_relations::tests::current_username_aliases_resolve_only_fresh_unique_owners -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --features manual-moderation --test manual_moderation -- --ignored
TEST_DATABASE_URL="$TEST_DATABASE_URL" run_test --test chat_db_mcp ask_mcp_client_starts_canonical_rmcp_child_with_env_clear_allowlist -- --ignored --exact
TEST_DATABASE_URL="$TEST_DATABASE_URL" TEST_DATABASE_URL_B="$TEST_DATABASE_URL_B" run_test --test chat_db_mcp independent_community_databases_generate_manifests_and_start_isolated_mcp_servers -- --ignored --exact
TEST_PRIVATE_HISTORY_DATABASE_URL_A="$TEST_DATABASE_URL" TEST_PRIVATE_HISTORY_DATABASE_URL_B="$TEST_DATABASE_URL_B" run_test --test private_url_history two_synthetic_databases_extract_only_their_scoped_messages_and_qr_hashes -- --ignored --exact
