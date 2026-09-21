# Community instance configuration

NedoBot runs one Telegram bot token as one community instance. A process may
own several managed chats in that community, but it does not contain a
multi-tenant registry of unrelated communities.

The LLM profile TOML must contain these non-secret sections:

```toml
[instance]
id = "spbvelo"
display_name = "Петербургское велообщество"
timezone = "Europe/Moscow"

[telegram]
unknown_chat_policy = "ignore"
owners = [5939287960]

[chats.general]
id = -1001111111111
ingest = true
moderation = true
stats = true
voice = false
ask = false
invite_url_env = "CHAT_INVITE_URL"
invite_label = "чате"

[chats.review]
id = -1004444444444
ingest = false
moderation = false
stats = false
voice = false
ask = false
review_destination = true

[moderation]
enabled = true
risk_profile = "ru_general_v1"
review_chat = "review"
reviewer_user_ids = [5939287960]
```

Chat invite configuration belongs to the chat entry, so `/ask` remains
independent from first-comment routes. `first_comment.routes[*].invite_url_env`
is retained as a per-route override for communities that use different public
links.

`[instance]`, `[telegram]` and at least one `[chats.*]` entry are required at
startup. Chat IDs must be unique. The bot ignores unknown group chats before
message, profile, reaction, member-event or audit persistence.

Commands and reports use the chat in which they were invoked as their default
scope. The response chat and the database scope are separate arguments in the
stats transport, so an admin-DM transport can be added without silently
changing the source chat.

`moderation.risk_profile` names a versioned policy. The profile can carry the
Telegram ID model metadata:

```toml
[risk_profiles.ru_general_v1]
version = "ru-general-v1"
old_user_message_threshold = 5
review_threshold = 70

[risk_profiles.ru_general_v1.telegram_id]
floor = 0.0066813
ceil = 1.0
k = 3.20435
midpoint_billion = 8.45919
version = "4pl-prod-2026-09-21"
```

Audit rows persist the selected profile and model versions. The configured
invite URL remains a secret/environment value; no Telegram invite or chat ID
is supplied by Rust defaults.

The current release profile keeps the existing Nedonews modules enabled by
default. A moderation-only community build can be checked with:

```bash
cargo check --no-default-features --features "moderation,spam-sync" --all-targets
```

If a config enables a module that is absent from the binary, startup fails
instead of silently ignoring that setting.
