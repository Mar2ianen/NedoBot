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

The endpoint and Nginx both enforce a 64 KiB request-body limit. The 70-second Nginx read timeout is deliberately longer than the 60-second application deadline, so clients receive the controlled application response. Tune the limits from access logs and DB pool saturation before raising them.
