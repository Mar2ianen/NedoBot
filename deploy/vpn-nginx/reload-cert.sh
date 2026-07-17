#!/bin/sh
set -eu

# Certbot deploy hook: Nginx in the host-networked container must reread
# renewed certificate files before serving them for the next TLS handshake.
podman exec vpn-nginx-decoy nginx -s reload
