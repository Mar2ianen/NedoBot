# T15 — Configuration, feature gates and release install

## Added verification

| Invariant | Minimal counterexample / oracle | Mutant caught | Layer |
|---|---|---|---|
| A configuration cannot enable a module missing from the binary. | The same valid synthetic community enables manual moderation and sets its global/chat gates. Default build accepts it; `--no-default-features` must reject it with `missing from this binary`. | Delete `require_compiled_feature("manual-moderation", …)`. The no-default test failed with `unwrap_err()` receiving `Ok(())`; restoring the guard makes it pass. | Real Rust test under both feature sets. |
| Feature-disabled builds remain compilable for the library. | Compile supported default and `--no-default-features` library configurations separately. | Feature-gated imports/types that only compile when default modules are present. | Cargo feature matrix script, also run in CI. |
| Release install staging does not partially replace either instance on an incomplete artifact, and concurrent complete installs leave both roots on the same release. | Existing temp-directory fixtures include all expected binaries under two synthetic install roots; omit one staged binary or race `alpha`/`beta` installers. | Move/swap files before preflight, skip one target, or allow concurrent mixed versions. | Existing Python installer tests, run in CI and locally. |

## Replay and verification

```sh
sh scripts/test_feature_matrix.sh
python3 tests/release_install.py
```

- Feature matrix: default check + no-default check + the config gate test under each build; all passed.
- Mutation: removing the compile-feature guard caused the no-default test to fail as expected. The mutation was restored and not committed.
- Release installer isolation/concurrency: 2 Python tests passed using temporary paths only.
- Existing config tests also cover invalid runtime secrets, duplicate managed chat IDs, missing/invalid instance configuration, dependency gates, and model/config validation. The full suite was run in T14 immediately before this matrix-only addition.
- `cargo fmt --all -- --check` and `git diff --check` passed.

## Worker lifecycle finding and correction

At the T15 test-stack base, long-lived workers were detached and there was no lifecycle contract to test. The separate refactor PR #144 introduced a shared supervisor; this audit found it logged unexpected worker exit but kept serving. A correction is now in PR #144 at head `8f46b26477fe117e2f6547280f750029bfefd9e4`: unexpected return, panic, and abort trigger shutdown, bounded drain, and a typed fatal result; normal dispatcher shutdown drains successfully. Eight worker lifecycle tests pass locally, as do the binary check and workspace Clippy. GitHub CI for that exact correction head is still running. Consequently, these tests validate a proposed refactor branch, not current `main`, and T15 is not fully verified until the correction is integrated and its dependency stack is validated.

No systemd restart behavior was asserted; installer tests are fully isolated from `/opt` and production paths. `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; root `AGENTS.md` and task-specific instructions were followed.
