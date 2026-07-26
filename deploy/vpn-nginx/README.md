# Nginx deployment for public NedoNews MCP

Before testing or reloading `nginx.conf`, install the rate-limit policy file:

```sh
install -D -m 0644 deploy/vpn-nginx/mcp-rate-limit.conf.example /etc/nedobot/nginx/mcp-rate-limit.conf
install -D -m 0644 deploy/vpn-nginx/mcp-rate-limit-location.conf.example /etc/nedobot/nginx/mcp-rate-limit-location.conf
nginx -t
systemctl reload nginx
```

The files under `/etc/nedobot/nginx/` are intentionally deployment configuration. Tune their `rate`, `burst`, and connection limit from access logs without changing the public MCP route or Rust service.

The committed starting policy is deliberately soft:

- 300 requests/minute/IP sustained;
- burst of 600 requests with `nodelay`;
- 40 simultaneous connections/IP.

The endpoint continues to enforce a 1 MiB request-body limit and upstream connect/send/read timeouts. Add a separate stricter zone for expensive tools only after access logs show that it is needed.
