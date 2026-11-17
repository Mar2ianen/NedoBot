#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <case-id> <seed>" >&2
    echo "cases: seeded-url-property deliberate-shrink-fixture utf16-generator telegram-update seeded-update-replay typed-state-generators reference-models durable-job-state-trace moderation-state-trace voice-media-bounds first-comment-render http-outcomes safe-report minimal-url-trace telegram-route-command-precedence ingress-reference-trace ask-tool-authorization ask-mcp-untrusted-preview ask-prefetch-budget" >&2
    exit 2
fi

case_id=$1
seed=$2
test_target_flag=--test
test_target=adversarial_harness
case "$seed" in
    ''|*[!0-9]*)
        echo "seed must be an unsigned decimal integer" >&2
        exit 2
        ;;
esac

case "$case_id" in
    seeded-url-property) test_filter=url_property_generator_replays_the_fixed_seed ;;
    deliberate-shrink-fixture) test_filter=deliberate_failure_shrinks_and_replays_identically ;;
    utf16-generator) test_filter=seeded_scope_and_utf16_generators_respect_their_contracts ;;
    telegram-update) test_filter=schema_valid_telegram_update_strategy_deserializes_as_teloxide_update ;;
    seeded-update-replay) test_filter=seeded_update_stream_replays_identically ;;
    typed-state-generators) test_filter=typed_authorization_and_ordered_job_strategies_round_trip ;;
    reference-models) test_filter=reference_models_reject_stale_job_completion_and_unauthorized_actions ;;
    durable-job-state-trace) test_filter=durable_job_reference_model_preserves_fences_and_uncertain_delivery ;;
    moderation-state-trace) test_filter=moderation_reference_model_preserves_veto_ownership_and_uncertainty ;;
    voice-media-bounds)
        test_target_flag=--lib
        test_target=
        test_filter=features::voice::download::tests::generated_media_metadata_matches_independent_size_and_duration_oracle
        ;;
    first-comment-render)
        test_target_flag=--lib
        test_target=
        test_filter=features::first_comment::render::tests::generated_unicode_and_control_text_never_becomes_untrusted_html
        ;;
    http-outcomes) test_filter=typed_http_outcomes_preserve_uncertain_delivery_and_bounds ;;
    safe-report) test_filter=reports_omit_payload_and_require_evidence_for_v1 ;;
    minimal-url-trace) test_filter=minimal_operation_trace_replays_with_independent_identity ;;
    telegram-route-command-precedence)
        test_target_flag=--bin
        test_target=tg_ai_bot_teloxide
        test_filter=structurally_valid_generated_messages_preserve_command_precedence
        ;;
    ingress-reference-trace) test_filter=ingress_reference_model_keeps_only_current_revision_and_media_evidence ;;
    ask-tool-authorization)
        test_target_flag=--lib
        test_target=
        test_filter=features::ask::agent::tests::generated_tool_authorization_matches_independent_allowlist_oracle
        ;;
    ask-mcp-untrusted-preview)
        test_target_flag=--lib
        test_target=
        test_filter=features::ask::agent::tests::generated_mcp_text_stays_untrusted_and_within_preview_budget
        ;;
    ask-prefetch-budget)
        test_target_flag=--lib
        test_target=
        test_filter=features::ask::agent::tests::reply_prefetch_does_not_exceed_the_tool_call_budget
        ;;
    *)
        echo "unknown quality case: $case_id" >&2
        echo "cases: seeded-url-property deliberate-shrink-fixture utf16-generator telegram-update seeded-update-replay typed-state-generators reference-models durable-job-state-trace moderation-state-trace voice-media-bounds first-comment-render http-outcomes safe-report minimal-url-trace telegram-route-command-precedence ingress-reference-trace ask-tool-authorization ask-mcp-untrusted-preview ask-prefetch-budget" >&2
        exit 2
        ;;
esac

export NEDOBOT_PROPTEST_SEED="$seed"
if [ -z "${GITHUB_SHA:-}" ]; then
    export NEDOBOT_TEST_SHA="$(git rev-parse HEAD)"
fi
if [ "$test_target_flag" = --lib ]; then
    exec cargo test --lib --locked "$test_filter" -- --nocapture
fi
exec cargo test "$test_target_flag" "$test_target" --locked "$test_filter" -- --nocapture
