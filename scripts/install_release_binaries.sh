#!/usr/bin/env bash
# Atomic install of prebuilt release binaries. Runs on the production host:
# binaries arrive in a staging dir, then each is swapped into
# /opt/tg-ai-bot-teloxide/target/release via temp file + rename, which is
# safe against the running executable (Linux ETXTBSY).
set -euo pipefail

STAGE_DIR="${1:?usage: install_release_binaries.sh <staged-artifact-dir>}"
DEST_DIR="/opt/tg-ai-bot-teloxide/target/release"
BINARIES=(
    tg_ai_bot_teloxide
    nedonews_mcp_http
    chat_db_mcp
    retry_pending_comments
    reconcile_comment_delivery
)

for binary in "${BINARIES[@]}"; do
    if [[ ! -x "${STAGE_DIR}/${binary}" ]]; then
        echo "missing executable in stage dir: ${binary}" >&2
        exit 1
    fi
    cp -f "${STAGE_DIR}/${binary}" "${DEST_DIR}/.${binary}.new"
    chmod 0755 "${DEST_DIR}/.${binary}.new"
    mv -f "${DEST_DIR}/.${binary}.new" "${DEST_DIR}/${binary}"
done

runuser -u tg-ai-bot -- test -x "${DEST_DIR}/tg_ai_bot_teloxide"
echo "installed: ${BINARIES[*]}"
