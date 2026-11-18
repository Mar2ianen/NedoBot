#!/bin/sh
set -eu

manifest=${QUALITY_EXECUTION_MANIFEST:-target/quality/executed-tests.tsv}
mkdir -p "$(dirname "$manifest")"
: > "$manifest"

# These selectors are pure or use local fake subprocesses; no database,
# Telegram API, external URL, model, or private corpus is required.
cases='utf16-generator telegram-update seeded-update-replay typed-state-generators reference-models durable-job-state-trace moderation-state-trace voice-media-bounds first-comment-render http-outcomes safe-report minimal-url-trace telegram-route-command-precedence ingress-reference-trace ask-tool-authorization ask-mcp-untrusted-preview mcp-sql-bindings mcp-forged-scope mcp-cursor-roundtrip audit-score-components memory-summary-normalization memory-error-classification agent-compaction-budget report-render-html-escape fetch-url-ip-literals qr-decoder-resource mcp-youtube-timeout media-writer-boundary'
seeds='72623859790382856 12345 20261118'
sha=${GITHUB_SHA:-$(git rev-parse HEAD)}

for seed in $seeds; do
    for case_id in $cases; do
        sh scripts/replay_quality_case.sh "$case_id" "$seed"
        printf '%s\t%s\t%s\n' "$sha" "$seed" "$case_id" | tee -a "$manifest"
    done
done
