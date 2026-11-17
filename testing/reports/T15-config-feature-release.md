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

## Unresolved worker lifecycle finding

The long-lived background jobs started from `src/main.rs` and several feature modules are detached with `tokio::spawn`; the dispatcher retains no handles or completion channel. A worker panic/return is therefore not observed by the application supervisor, and there is no bounded drain contract on shutdown. The repo currently has no worker-supervisor API to exercise with panic/abort/return and virtual-time shutdown tests. I have not added a mock-only test that would imply this runtime guarantee exists. This requirement remains open and needs a separate production change plus tests before T15 is complete.

No systemd restart behavior was asserted; installer tests are fully isolated from `/opt` and production paths. `docs/CODE_STYLE.md` and `docs/TEST_QUALITY.md` are absent in this checkout; root `AGENTS.md` and task-specific instructions were followed.
