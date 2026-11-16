# Refactor release gates

These gates define evidence before any release. This refactor work does not merge or deploy itself.

## 1. Review and CI preflight

- Confirm the reviewed PR head equals the source commit intended for release and that required CI is green on that exact SHA.
- Require formatting, locked workspace check/test, clippy, no-default test/check, release installer checks, and PostgreSQL migration/integration lanes. Record actual passed/failed/ignored counts; a filtered test run is not a substitute for the full lane.
- Review migration SQL as additive or explicitly reversible. Verify a clean-database install and an existing-database upgrade on disposable PostgreSQL copies. Do not rewrite applied history.
- Check changes against the instance/config boundary, URL fetch policy, private MCP scope, moderation authority, typed delivery certainty and feature gates.
- Capture `git diff --check`, source hash, workflow URL and immutable candidate SHA in the release record.

## 2. Release artifact and staging

- Build the release artifact once from the approved source SHA. Record toolchain/target and SHA-256; do not rebuild during install without generating a new record.
- Validate both community profiles against the candidate binary and their own environment without printing secrets. Verify each database's migration ledger separately.
- Staging is opt-in and isolated from production credentials, databases and Telegram chats. Smoke both instance configurations, MCP own-scope and forged-scope rejection, update routing, an ordinary chat message, a test-only worker failure/restart, and a controlled delivery-unknown case.
- Observe service restart count, journal errors, job backlog, lease recovery, Telegram delivery errors, decision/audit latency and MCP latency through the chosen staging window. Compare against recorded baseline and stop on unexplained growth or cross-instance access.

## 3. Production install and health

- Production deploy requires a merged `main` commit, immutable annotated `deploy-YYYY-MM-DD-scope` tag, approved release artifact, saved checksums, and an explicit operator-triggered release workflow. No feature branch or draft PR is a deploy source.
- Before install, back up the previous binaries and both runtime profile files; preserve credentials and make no implicit profile edits. Apply additive migrations independently to each database and capture their version/ledger evidence.
- Restart and verify all production services for both instances, compare running executable hashes to the artifact, check `NRestarts`, and inspect each journal for migration/runtime errors. Run instance-scoped read-only MCP smoke tests and verify public MCP exposure remains separately gated.
- Keep release and SSH/manual health evidence distinct. Report the actual observation duration; a one-minute check must not be described as a ten-minute window.

## 4. Rollback

- If binary-only behavior regresses, restore the exact backed-up binaries, restart all affected services, and recheck hashes/health. Preserve current profiles and databases unless a separately reviewed migration rollback says otherwise.
- For additive schema changes, first stop affected workers, then use only a migration-specific rollback or forward-fix reviewed for data preservation. Never drop/restore a shared production database as a generic rollback.
- Record rollback source SHA, artifact checksums, service state, database migration versions, and any queued `delivery_unknown` jobs that require operator reconciliation.

## No automatic deployment

CI success, PR readiness, staging smoke, and release preparation do not imply approval to merge or deploy. Production credentials and config remain operator-owned.
