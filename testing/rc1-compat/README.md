# Actual RC1/RC2 storage compatibility

This independent Cargo workspace deliberately includes two revisions of teloxide-core.
It must not be added to the production workspace. The test uses the real RC1 Message
decoder on RC2 dual-write output, then feeds the RC1 rewrite back to RC2.

`Cargo.lock` is intentionally independent from the production workspace because
both teloxide-core revisions must be present at once. Run it reproducibly with
`cargo test --manifest-path testing/rc1-compat/Cargo.toml --locked`; regenerate
the lockfile only when changing one of these pinned dependencies. CI runs this
compatibility check as a separate workspace.
This is a storage compatibility test, not evidence of RC1 network compatibility
with a live Bot API 10.5 endpoint.
