#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <case-id> <seed>" >&2
    echo "cases: seeded-url-property deliberate-shrink-fixture utf16-generator telegram-update seeded-update-replay typed-state-generators reference-models http-outcomes safe-report minimal-url-trace" >&2
    exit 2
fi

case_id=$1
seed=$2
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
    http-outcomes) test_filter=typed_http_outcomes_preserve_uncertain_delivery_and_bounds ;;
    safe-report) test_filter=reports_omit_payload_and_require_evidence_for_v1 ;;
    minimal-url-trace) test_filter=minimal_operation_trace_replays_with_independent_identity ;;
    *)
        echo "unknown quality case: $case_id" >&2
        echo "cases: seeded-url-property deliberate-shrink-fixture utf16-generator telegram-update seeded-update-replay typed-state-generators reference-models http-outcomes safe-report minimal-url-trace" >&2
        exit 2
        ;;
esac

export NEDOBOT_PROPTEST_SEED="$seed"
if [ -z "${GITHUB_SHA:-}" ]; then
    export NEDOBOT_TEST_SHA="$(git rev-parse HEAD)"
fi
exec cargo test --test adversarial_harness --locked "$test_filter" -- --nocapture
