# NedoNews RMCP public canary

These artifacts start the RMCP Streamable HTTP server alongside the legacy public MCP server. They **do not** modify the existing public route:

| Service | Local listener | External path |
| --- | --- | --- |
| Legacy `nedonews-mcp.service` | `127.0.0.1:8787` | `/mcp/nedonews` |
| RMCP canary `nedonews-mcp-rmcp-canary.service` | `127.0.0.1:8788` | `/mcp/nedonews-rmcp-canary` |

The canary reuses the existing narrowly scoped read-only PostgreSQL role and MCP manifest. It has a distinct environment file so an RMCP configuration change cannot alter the legacy process.

## Rollout

1. Build the already-present RMCP binary from the release being deployed:

   ```sh
   cd /opt/tg-ai-bot-teloxide
   cargo build --release --bin nedonews_mcp_http_rmcp
   ```

2. Install the separate environment file and replace `REDACTED` with the existing read-only role password. Restrict it to the service user:

   ```sh
   install -D -o tg-ai-bot -g tg-ai-bot -m 0600 \
     deploy/nedonews-mcp/nedonews-mcp-rmcp-canary.env.example \
     /etc/nedobot/nedonews-mcp-rmcp-canary.env
   editor /etc/nedobot/nedonews-mcp-rmcp-canary.env
   ```

   Keep `MCP_BIND=127.0.0.1:8788`, `MCP_PATH=/mcp/nedonews-rmcp-canary`, and the exact `MCP_ALLOWED_HOSTS`/`MCP_ALLOWED_ORIGINS` allowlists unless the externally served domain or trusted browser client changes.

3. Install and start the sidecar unit. This does not restart `nedonews-mcp.service`:

   ```sh
   install -D -m 0644 deploy/nedonews-mcp/nedonews-mcp-rmcp-canary.service \
     /etc/systemd/system/nedonews-mcp-rmcp-canary.service
   systemctl daemon-reload
   systemctl enable --now nedonews-mcp-rmcp-canary.service
   systemctl status --no-pager nedonews-mcp-rmcp-canary.service
   ```

4. Smoke-test the loopback canary before publishing it. The Host header is required by the RMCP allowlist:

   ```sh
   curl --fail-with-body \
     -H 'Host: nedobot.chickenkiller.com' \
     -H 'Content-Type: application/json' \
     --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"canary-smoke","version":"1"}}}' \
     http://127.0.0.1:8788/mcp/nedonews-rmcp-canary
   ```

   Follow this with the RMCP client, MCP Inspector, and read-only `search`, `select`, `aggregate`, pagination, and error-path checks required by RMCP-10.

5. Publish only the new canary route after the local smoke test. Copy the location block from `../vpn-nginx/nedonews-mcp-rmcp-canary-location.conf.example` into the existing TLS `server` for `nedobot.chickenkiller.com`, then validate and reload:

   ```sh
   nginx -t
   systemctl reload nginx
   ```

   Verify `https://nedobot.chickenkiller.com/mcp/nedonews-rmcp-canary`. Do not edit the legacy `location = /mcp/nedonews` block or change its `8787` upstream during this canary phase.

## Rollback

If the **external canary route** is problematic, remove only the new `/mcp/nedonews-rmcp-canary` location block, then run:

```sh
nginx -t && systemctl reload nginx
```

If the **sidecar process** is problematic, stop and disable only the canary:

```sh
systemctl disable --now nedonews-mcp-rmcp-canary.service
```

The legacy `nedonews-mcp.service` on `127.0.0.1:8787` and `/mcp/nedonews` remains running throughout both actions. Do not switch the legacy Nginx upstream to `8788` as part of this rollout.
