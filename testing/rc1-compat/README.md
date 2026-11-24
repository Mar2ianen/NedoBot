# Actual RC1/RC2 storage compatibility

This independent Cargo workspace deliberately includes two revisions of teloxide-core.
It must not be added to the production workspace. The test uses the real RC1 Message
decoder on RC2 dual-write output, then feeds the RC1 rewrite back to RC2.

First run: `cargo generate-lockfile --manifest-path testing/rc1-compat/Cargo.toml`.
Review and commit that independent lockfile, then run
`cargo test --manifest-path testing/rc1-compat/Cargo.toml --locked`.
The lockfile has not been generated or tested in the connector-only environment.
This is a storage compatibility test, not evidence of RC1 network compatibility
with a live Bot API 10.5 endpoint.
