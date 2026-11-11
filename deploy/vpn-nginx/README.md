# Nginx deployment for public NedoNews MCP

`vpn-nginx-decoy` монтирует только `nginx.conf` в контейнер. Rate-limit policy находится в этом файле рядом с public MCP route, поэтому перед reload достаточно проверить конфигурацию внутри контейнера:

```sh
podman exec vpn-nginx-decoy nginx -t
podman exec vpn-nginx-decoy nginx -s reload
```

Настраивать `rate`, `burst` и connection limit нужно в `deploy/vpn-nginx/nginx.conf`, затем выкладывать именно этот mounted config.

The committed starting policy is deliberately soft:

- 300 requests/minute/IP sustained;
- burst of 600 requests with `nodelay`;
- 40 simultaneous connections/IP;
- 8 simultaneous connections across the public MCP virtual host.

The endpoint and Nginx both enforce a 64 KiB request-body limit. Nginx retains the 15-second body timeout and the existing connection/rate limits. The 650-second upstream read timeout covers the 630-second application deadline (at most five 120-second subtitle downloads plus overhead). PostgreSQL statement and lock timeouts stay unchanged. MCP clients may impose a shorter timeout independently; transcript pagination keeps cached continuation requests short.
