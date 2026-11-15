# Private corpus and logs

Historical links from NedoNews and PVO require explicitly approved read-only access to their separate databases; never assume a network connector authorizes copying data into the repo. No network requests to values from chat, no Telegram bot posts, no live feeds.

Maintain `testing/corpus/public` with only synthetic values. Store private material outside working tree, in a private directory with restrictive permissions and nonshared filesystem. Prefer HMAC-SHA256 with an external secret for private equality analysis; plain SHA256 can be dictionary-attacked. No private sample in CI, PR, GitHub artifact, stacktrace, test failure shrinking logs, metrics labels, or debug prints. Convert minimized failure to synthetic case before publication and independently run a secret scan. If valid reproduction impossible without disclosing the sensitive input, report abstract shape and keep local.
