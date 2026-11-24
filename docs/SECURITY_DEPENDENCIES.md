# Dependency security notes

## SQLite and RUSTSEC-2026-0459

The spam reputation store remains SQLite. SQLx 0.9.0 currently constrains
`libsqlite3-sys` to `>=0.30.1, <0.38.0`; RustSec's fix is in
`libsqlite3-sys >=0.39.0`, so the supported SQLx release cannot yet select the
patched bundled SQLite. The locked dependency is `libsqlite3-sys 0.37.0`, whose
bundled SQLite source reports version 3.51.3. The `sqlite` SQLx feature statically
bundles that source; it does not use the host SQLite library.

The narrow `deny.toml` exception for `RUSTSEC-2026-0459` is temporary and does
not patch SQLite. The advisory describes disclosure when a process opens a
database together with a crafted sibling WAL file. Telegram content cannot
choose or write the journal path. On the production VPS, read-only inspection
found `/var/lib/nedobot` mode `0700` and the SQLite database and WAL mode `0600`,
owned by root; both bot services run with systemd's default root user. This
limits file creation to the trusted service account. This is a reduced local
attack surface, not a claim that the bundled SQLite is patched.

The VPS system SQLite was `3.45.1`, below the advisory's fixed `3.54.0` line, so
switching this build to dynamic system linking would not currently fix the issue
and would introduce a host-library runtime dependency. Keep the single advisory
exception until SQLx accepts a patched `libsqlite3-sys` release or the production
system library is upgraded under a separately approved infrastructure change.
Recheck `cargo tree -i libsqlite3-sys -e features`, linked SQLite mode/version,
and `cargo deny check advisories` when that becomes possible. No other advisory
is ignored here.

## URL and Chrono advisories (2026-11-24)

- [RUSTSEC-2026-0592](https://rustsec.org/advisories/RUSTSEC-2026-0592.html): `url` before 2.5.9 can overflow the stack in `Url::origin()` on nested `blob:` URLs. Update the lockfile with `cargo update -p url --precise 2.5.9` (or a later compatible patched release) and rerun the dependency audit and URL regressions.
- [RUSTSEC-2026-0543](https://rustsec.org/advisories/RUSTSEC-2026-0543.html): `chrono` is unmaintained; there is no fixed version. Inventory direct usage and upstream dependencies with `cargo tree -i chrono -e features` before planning a migration to `jiff` or `time`. Preserve database timestamp semantics and timezone behavior. Do not silently suppress the advisory or claim that the URL update alone clears the audit.
