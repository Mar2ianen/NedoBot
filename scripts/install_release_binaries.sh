#!/usr/bin/env bash
# Atomic install of prebuilt release binaries. Runs on the production host:
# binaries arrive in a staging dir, then each is swapped into
# /opt/tg-ai-bot-teloxide/target/release via temp file + rename, which is
# safe against the running executable (Linux ETXTBSY).
set -euo pipefail

BINARIES=(
    tg_ai_bot_teloxide
    nedonews_mcp_http
    chat_db_mcp
    migrate
    provision_mcp_instance_scope
    generate_mcp_db_manifest
    validate_instance_config
    probe_new_user_audit
    spam_cascade
    spam_decision
    retry_pending_comments
    reconcile_comment_delivery
    backfill_rich_messages
    backfill_post_history_embeddings
    backfill_audit_embeddings
    backfill_chat_embeddings
    import_telegram_export
    probe_ask_route
)

if [[ "${1:-}" == --list-binaries ]]; then
    printf '%s\n' "${BINARIES[@]}"
    exit
fi

restore_release() {
    local backup="${1:?usage: install_release_binaries.sh --restore <verified-backup-dir>}"
    local release_root="${NEDOBOT_RELEASE_ROOT:-/opt}"
    local lock_path="${NEDOBOT_RELEASE_LOCK:-/var/lock/nedobot-release.lock}"
    local -a dest_dirs=(
        "$release_root/tg-ai-bot-teloxide/target/release"
        "$release_root/nedobot-pvo/target/release"
    )
    local -a temp_dirs=()

    exec 9>"$lock_path"
    flock 9
    (
        cd -- "$backup"
        sha256sum --strict --check --status SHA256SUMS
    )
    test -f "$backup/llm_profiles.toml"
    test -f "$backup/pvo-llm_profiles.toml"
    for dest_dir in "${dest_dirs[@]}"; do
        local instance="${dest_dir#"$release_root"/}"
        instance="${instance%%/*}"
        test -f "$backup/$instance/.env"
        test -d "$dest_dir"
        temp_dir="$(mktemp -d "$dest_dir/.rollback.XXXXXXXX")"
        temp_dirs+=("$temp_dir")
        for binary in "${BINARIES[@]}"; do
            if [[ -f "$backup/ABSENT" ]] \
                && grep -Fxq "$instance/$binary" "$backup/ABSENT"; then
                continue
            fi
            test -x "$backup/$instance/$binary"
            cp -p -- "$backup/$instance/$binary" "$temp_dir/$binary"
        done
        cp -p -- "$backup/$instance/.env" "$temp_dir/.env"
    done

    for index in "${!dest_dirs[@]}"; do
        local dest_dir="${dest_dirs[$index]}"
        local instance="${dest_dir#"$release_root"/}"
        instance="${instance%%/*}"
        local temp_dir="${temp_dirs[$index]}"
        for binary in "${BINARIES[@]}"; do
            if [[ -f "$backup/ABSENT" ]] \
                && grep -Fxq "$instance/$binary" "$backup/ABSENT"; then
                rm -f -- "$dest_dir/$binary"
            else
                mv -f -- "$temp_dir/$binary" "$dest_dir/$binary"
            fi
        done
        mv -f -- "$temp_dir/.env" "$release_root/$instance/.env"
        rmdir -- "$temp_dir"
    done
    echo "restored binaries and environment files from verified backup: $backup"
}

if [[ "${1:-}" == --restore ]]; then
    [[ "$#" -eq 2 ]] || { echo "usage: $0 --restore <verified-backup-dir>" >&2; exit 2; }
    restore_release "$2"
    exit
fi

STAGE_DIR="${1:?usage: install_release_binaries.sh <staged-artifact-dir> [--backup]}"
RELEASE_ROOT="${NEDOBOT_RELEASE_ROOT:-/opt}"
LOCK_PATH="${NEDOBOT_RELEASE_LOCK:-/var/lock/nedobot-release.lock}"
exec 9>"$LOCK_PATH"
flock 9
DEST_DIRS=(
    "$RELEASE_ROOT/tg-ai-bot-teloxide/target/release"
    "$RELEASE_ROOT/nedobot-pvo/target/release"
)

for binary in "${BINARIES[@]}"; do
    test -x "${STAGE_DIR}/${binary}" || { echo "missing executable: $binary" >&2; exit 1; }
done
if [[ "${2:-}" == --backup ]]; then
    backup_root="${NEDOBOT_BACKUP_ROOT:-/var/backups/nedobot}"
    config_root="${NEDOBOT_CONFIG_ROOT:-/etc/tg-ai-bot}"
    umask 077
    install -d -m 0700 -- "$backup_root"
    backup="$(mktemp -d "$backup_root/release.XXXXXXXX")"
    backup_complete=false
    cleanup_partial_backup() {
        if [[ -n "${backup:-}" && "$backup_complete" != true ]]; then
            rm -rf -- "$backup"
        fi
    }
    trap cleanup_partial_backup EXIT
    for profile in llm_profiles.toml pvo-llm_profiles.toml; do
        install -m 0600 -- "$config_root/$profile" "$backup/$profile"
        cmp --silent -- "$config_root/$profile" "$backup/$profile"
    done
    for dest_dir in "${DEST_DIRS[@]}"; do
        instance="${dest_dir#"$RELEASE_ROOT"/}"
        instance="${instance%%/*}"
        install -d -m 0700 -- "$backup/$instance"
        cp -pL -- "$RELEASE_ROOT/$instance/.env" "$backup/$instance/.env"
        cmp --silent -- "$RELEASE_ROOT/$instance/.env" "$backup/$instance/.env"
        test -x "$dest_dir/tg_ai_bot_teloxide"
        for binary in "${BINARIES[@]}"; do
            if [[ -e "$dest_dir/$binary" || -L "$dest_dir/$binary" ]]; then
                cp -pL -- "$dest_dir/$binary" "$backup/$instance/$binary"
                cmp --silent -- "$dest_dir/$binary" "$backup/$instance/$binary"
            else
                printf '%s\n' "$instance/$binary" >> "$backup/ABSENT"
            fi
        done
    done
    (
        cd "$backup"
        find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
        sha256sum --check --status SHA256SUMS
    )
    backup_complete=true
    trap - EXIT
    echo "verified rollback backup: $backup"
fi
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
