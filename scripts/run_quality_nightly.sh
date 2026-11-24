#!/bin/sh
set -eu

export QUALITY_EXECUTION_MANIFEST=${QUALITY_EXECUTION_MANIFEST:-target/quality/executed-tests.tsv}
mkdir -p "$(dirname "$QUALITY_EXECUTION_MANIFEST")"
printf 'sha\tcase\tseed\ttarget\ttest\tpassed\n' > "$QUALITY_EXECUTION_MANIFEST"

# These selectors are pure or use local fake subprocesses; no database,
# Telegram API, external URL, model, or private corpus is required.
cases='seeded-url-property stored-rich-quotes utf16-generator telegram-update seeded-update-replay typed-state-generators reference-models durable-job-state-trace moderation-state-trace voice-media-bounds first-comment-render http-outcomes safe-report minimal-url-trace telegram-route-command-precedence ingress-reference-trace ask-tool-authorization ask-mcp-untrusted-preview mcp-sql-bindings mcp-forged-scope mcp-cursor-roundtrip audit-score-components memory-summary-normalization memory-error-classification agent-compaction-budget report-render-html-escape fetch-url-ip-literals qr-decoder-resource mcp-youtube-timeout media-writer-boundary'
seeds='72623859790382856 12345 20261118'
for seed in $seeds; do
    for case_id in $cases; do
        case "$case_id" in
            reference-models|safe-report|minimal-url-trace|telegram-route-command-precedence|ask-tool-authorization|ask-mcp-untrusted-preview|fetch-url-ip-literals|mcp-youtube-timeout|media-writer-boundary)
                [ "$seed" = 72623859790382856 ] || continue ;;
        esac
        sh scripts/replay_quality_case.sh "$case_id" "$seed"
    done
done
