# CI lanes and objective gates

**PR fast (every change):** format, feature matrix, test corpus, deterministic proptest (start 256–2048 cases/property), checked `cargo test` counts; small synthetic PG/SQLite integrations under existing test service. Do not invoke daily private corpus on GitHub Actions.

**Daily/nightly extended:** 50k–200k properties or a timed budget after profiling, randomized state traces, deterministic failure schedules, fresh fuzz corpus. Isolate heavy libFuzzer jobs. Record exact toolchain/build SHA, seed, target, cases, unique edges, timeouts.

**Weekly/security:** structured libFuzzer targets, fuzz corpus minimization, cargo-llvm-cov for branch visibility, cargo-mutants for semantic effectiveness. Don't optimize for bare coverage percentage.

**Zero-test check:** `cargo test --exact` can exit 0 with `0 passed`; CI must fail if selected test count is zero. Compare stdout/stderr `test result` counts, or use tests with explicit expected marker. A green CI without proof isn't acceptance.

**Cross-process SQLite:** use distinct OS processes where intended, not merely two pools in one process. Ensure WAL and locked file are in a disposable temporary directory, robust cleanup.

**Expected constraints:** none of these tasks authorizes live Telegram updates, web probes of user links, production deploy, spam enforcement, schema migrations or replacement of SQLite. Known RustSec exceptions stay documented, not hidden.
