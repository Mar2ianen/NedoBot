# T04 — shared runtime config and voice settings boundary

| Field | Result |
|---|---|
| Baseline | `main` SHA `7b8815bbff80c4665b64076a063f5f4919e5465a`; stacked after T00/#133, T01/#134, T02/#135 and T03/#136 |
| Branch / PR | `codex/refactor-t04-shared-config`; [draft PR #137](https://github.com/Mar2ianen/NedoBot/pull/137), head `4efe969f8d3a13e237d284ec1f67df491265e206`, stacked on #136 |
| Rust API changes | `AppState.config` is now `Arc<Config>`; voice runtime settings are grouped as `Config.voice: VoiceConfig`. Worker startup APIs share the same `Arc<Config>`. |
| Config compatibility | `RuntimeSettings` fields and serialized TOML/env names remain unchanged. `Config::from_env` maps the same parsed values into the internal `VoiceConfig`; community and secondary-chat runtime gates are unchanged. |
| Dependencies / SQL | No dependency, migration, SQL or production configuration change. |

## Verification

- `cargo fmt --all -- --check`, `git diff --check` — passed.
- `cargo test --workspace --all-targets --locked` — 665 passed, 0 failed, 13 ignored, 0 filtered.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` — passed.
- `cargo check --workspace --all-targets --no-default-features --locked` — passed.
- `app_state_clones_share_the_same_runtime_config_allocation` confirms cloned state points to the same `Arc<Config>` allocation without connecting to PostgreSQL.
- Existing config-file parser tests cover the legacy flat voice keys and defaults; the full suite includes the committed production community profile validation.
- `./scripts/test.sh` was not needed: this change does not affect database behavior or schema.

## Invariants and remaining scope

- `AppState::clone()` now clones the config pointer; long-lived URL-feed, decision-audit and profile-refresh workers receive clones of the same pointer.
- `Config` remains immutable behind `Arc`; no global mutable configuration was introduced.
- Compile-time, instance/runtime, and per-chat feature gates remain distinct and are documented in `COMMUNITY_CONFIG.md`; explicit invalid environment values continue to fail startup validation.
- This slice groups VoiceConfig only, as required by the one-domain-at-a-time constraint. Ask, moderation and URL config grouping and narrower feature dependency structs remain future T04 follow-up slices; no bulk field rename was attempted.
- `AppState.config` and `Config.voice` are Rust API shape changes. In-repository callers and test fixtures compile; out-of-repository consumers constructing these public structs would need to adapt.
