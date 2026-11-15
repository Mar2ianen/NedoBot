# Triage routine

1. Freeze code SHA, Rust toolchain, PG/SQLite versions, random seed/corpus hash.
2. Validate source and V1/V2/V3; distinguish actual Telegram user input from internal synthetic DB operation.
3. Derive minimal repro by shrinking op list first, values second. Use deterministic schedule for concurrency; logs must not include original personal data.
4. Show failing assertion before fix; if cannot reproduce twice, label suspected/flaky not confirmed.
5. Categorize severity: data privacy/identity misattribution, false enforcement, loss of updates, stale projections, DOS/resource exhaustion, harmless parser divergence.
6. Add a deterministic public synthetic regression, or only private protected one if synthetic adaptation changes semantics. Describe why.
7. Check tests actually executed; attach only sanitized outputs.
