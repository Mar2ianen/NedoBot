# Independent invariants (IDs for reports)

| ID | Invariant |
|---|---|
| URL-01 | Canonical target independent of VisibleText/TelegramEntity/RichMessage/QR source |
| URL-02 | Duplicate same semantic URL doesn't increase unique count; distinct path/query case not collapsed |
| URL-03 | Lookalike domain never classified as Telegram-owned domain |
| URL-04 | Resolver never assigns current peer ID to ambiguous/stale username alias |
| URL-05 | Unresolved is not known external, bot suffix isn't `is_bot` |
| ENT-01 | Valid UTF-16 offsets preserve entity boundaries and original peer ID |
| ENT-02 | Hidden link target is not replaced by visible display text |
| UPD-01 | Duplicate update is idempotent for persistent projections |
| UPD-02 | Edit/retry/delete/QR stale event cannot resurrect superseded evidence |
| QR-01 | Decode bounded in compressed bytes, dimensions, aggregate scan work, candidates, payloads |
| QR-02 | Two distinct QR URLs stay distinct and identical payloads dedupe |
| EXP-01 | Default export hides every arbitrary path/query/fragment/userinfo and invite/auth value |
| EXP-02 | Aggregation uses full target identity not redacted text; no silent truncate |
| EXP-03 | Human spam/not-spam metadata distinct from `is_spammer` and `legacy_backfill` |
| SHD-01 | Quiescent shadow equals current evidence; removal produces zero state |
| SHD-02 | Shadow never affects production ban/review/captcha/enforcement |
| JOB-01 | Only current lease/generation can materialize results |
| PG-01 | Same user URL-refresh serialized; different users can progress concurrently |
| PG-02 | Cancellation/panic returns no session advisory lock to pool |
| SQL-01 | SQLite two-process writes don't lose decisions/retractions; restart durable |
| ISO-01 | NedoNews/PVO PG data and MCP read rights cannot leak across instances |
| FEED-01 | Untrusted URL never triggers unapproved network fetch/redirect/SSRF |
| SYS-01 | Failures bounded in CPU/memory/time; reproducible deterministic seed |

These invariants are **acceptance goals**, not claims that current bot satisfies all of them. Cases where a rule is domain-specific must be clarified before adding an assertion.
