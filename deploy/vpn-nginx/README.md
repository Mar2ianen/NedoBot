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
- 40 simultaneous connections/IP;
- 8 simultaneous connections across the public MCP virtual host.

The endpoint and Nginx both enforce a 64 KiB request-body limit. The 70-second Nginx read timeout is deliberately longer than the 60-second application deadline, so clients receive the controlled application response. Tune the limits from access logs and DB pool saturation before raising them.
