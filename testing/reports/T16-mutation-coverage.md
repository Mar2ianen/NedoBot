# T16 — Targeted mutation and coverage baseline

## Reproducible tools

Long-running checks are separate from the fast CI lane. The manually installed tools were:

- `cargo-mutants 27.1.0`
- `cargo-llvm-cov 0.9.1`
- system `llvm-cov` / `llvm-profdata` 23.1.5
- `cargo-fuzz` and a nightly Rust toolchain are not installed; no sanitizer fuzz run is claimed.

Scripts pin expected tool versions in their install guidance and cap mutation concurrency/build debug output. They are not part of the fast CI workflow.

```sh
cargo install cargo-mutants --version 27.1.0 --locked
cargo install cargo-llvm-cov --version 0.9.1 --locked
sh scripts/run_targeted_mutations.sh
LLVM_COV="$(command -v llvm-cov)" LLVM_PROFDATA="$(command -v llvm-profdata)" sh scripts/run_coverage.sh
```

## Mutation results

| Mutant | Outcome | Responsible oracle |
|---|---|---|
| `src/features/search/mcp.rs:423` replace `is_safe_fetch_url` with `true` | Killed | `fetch_url_filter_rejects_private_and_non_http_targets` and `fetch_urls_never_forwards_blocked_ip_literals_and_keeps_lookalike_hosts_distinct` |
| `src/features/search/mcp.rs:423` replace `is_safe_fetch_url` with `false` | Killed | `fetch_url_filter_accepts_public_http_targets` and mixed URL forwarding test |

The first broad invocation built every target for each mutation and failed at link time with `LLVM ERROR: IO failure on output stream: Disk quota exceeded`; those two mutants were classified **unviable**, not killed. Restricting the mutation test to `--lib`, one job and no debug symbols produced the 2/2 killed result above. Mutant reports are machine-local (`mutants.out/`) and intentionally not committed.

Related concrete mutants killed in earlier task reports: reversed stats user tie-break (`T13`), broad retry-message substring classifier (`T12`), and removed config compile-feature guard (`T15`). See those reports for their independent counterexamples and replay commands.

## Coverage baseline

`cargo llvm-cov --workspace --all-targets --locked --summary-only` completed on the default feature set:

- lines: **42.25%** (23,300 covered / 55,145 total)
- regions: **47.95%**
- functions: **45.91%**
- branch coverage: not collected; LLVM branch coverage is unstable in this stable toolchain.

This is a baseline, not a gate. `--all-targets` leaves ignored PostgreSQL tests ignored; run `scripts/test.sh` for disposable-DB lanes. The report showed especially low line coverage in `telegram/command_handler.rs` (21.88%) and `telegram/manual_moderation.rs` (24.85%) because their non-DB paths and production adapter branches dominate the source file. These are risk indicators, not a claim that existing focused tests provide no coverage.

## Fuzzing and remaining risks

- No `cargo-fuzz` target was added: the environment lacks both cargo-fuzz and nightly, so a new target could not be compiled or run here. Existing seeded, shrinking generators remain the executable adversarial lane for schema-valid Telegram updates, QR images, URL cases, reports and state traces.
- No global 90%/95% coverage threshold was introduced. Future targets should add fuzzing only for pure parsers with bounded inputs and semantic assertions, and should promote every discovered seed into an ordinary regression test.
- Coverage does not prove branch quality or mutation adequacy outside the two selected URL policy mutants. T15 worker supervision is still an open runtime gap and should receive lifecycle mutants after that supervisor exists.
