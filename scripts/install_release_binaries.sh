#!/usr/bin/env bash
# Atomic install of prebuilt release binaries. Runs on the production host:
# binaries arrive in a staging dir, then each is swapped into
# /opt/tg-ai-bot-teloxide/target/release via temp file + rename, which is
# safe against the running executable (Linux ETXTBSY).
set -euo pipefail

STAGE_DIR="${1:?usage: install_release_binaries.sh <staged-artifact-dir>}"
RELEASE_ROOT="${NEDOBOT_RELEASE_ROOT:-/opt}"
LOCK_PATH="${NEDOBOT_RELEASE_LOCK:-/var/lock/nedobot-release.lock}"
exec 9>"$LOCK_PATH"
flock 9
# Both community instances run the same binaries from their own checkouts.
DEST_DIRS=(
    "$RELEASE_ROOT/tg-ai-bot-teloxide/target/release"
    "$RELEASE_ROOT/nedobot-pvo/target/release"
)
BINARIES=(
    tg_ai_bot_teloxide
    nedonews_mcp_http
    chat_db_mcp
    retry_pending_comments
    reconcile_comment_delivery
    backfill_rich_messages
)

for binary in "${BINARIES[@]}"; do
    test -x "${STAGE_DIR}/${binary}" || { echo "missing executable: $binary" >&2; exit 1; }
done
TEMP_DIRS=()
cleanup() { for temp_dir in "${TEMP_DIRS[@]}"; do rm -rf -- "$temp_dir"; done; }
trap cleanup EXIT
# Все копии готовы до первой замены. Уникальная директория исключает
# совместную запись в inode исполняемого файла при конкурирующих installs.
for dest_dir in "${DEST_DIRS[@]}"; do
    test -d "$dest_dir"
    temp_dir="$(mktemp -d "$dest_dir/.release.XXXXXXXX")"
    TEMP_DIRS+=("$temp_dir")
    for binary in "${BINARIES[@]}"; do
        install -m 0755 "${STAGE_DIR}/${binary}" "$temp_dir/$binary"
    done
done
for index in "${!DEST_DIRS[@]}"; do
    for binary in "${BINARIES[@]}"; do
        mv -f "${TEMP_DIRS[$index]}/$binary" "${DEST_DIRS[$index]}/$binary"
    done
done

if [[ "$RELEASE_ROOT" == /opt ]]; then
    for dest_dir in "${DEST_DIRS[@]}"; do
        runuser -u tg-ai-bot -- test -x "$dest_dir/tg_ai_bot_teloxide"
    done
fi
echo "installed to: ${DEST_DIRS[*]}: ${BINARIES[*]}"
