# Bounded coverage-guided fuzzing

This is an independent cargo-fuzz workspace, never a production dependency.
Generate and review its Cargo.lock before the first run. Keep the teloxide-url
pin in sync with the root workspace (including after antispam #9 merges).

Run in a disposable environment without production tokens or database access:
```sh
cargo +nightly fuzz run stored_message -- -max_total_time=60 -max_len=65536 -rss_limit_mb=2048 -timeout=10
cargo +nightly fuzz run qr_image -- -max_total_time=60 -max_len=262144 -rss_limit_mb=2048 -timeout=10
cargo +nightly fuzz run url_parse -- -max_total_time=60 -max_len=16384 -rss_limit_mb=2048 -timeout=10
```
Seed the image corpus with synthetic QR fixtures, not screenshots of real chats.
Keep minimized synthetic failures and replay commands. Do not upload private
payloads. Targets are not evidence of an executed sanitizer run.
