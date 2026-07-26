#!/bin/sh
set -eu

CONTAINER_NAME=tg-ai-bot-postgres-dev
VOLUME_NAME=tg-ai-bot-postgres-dev-data
IMAGE=pgvector/pgvector:0.8.2-pg16-bookworm
DEV_DATABASE=tg_ai_bot_dev
TEST_DATABASE=tg_ai_bot_test
DATABASE_USER=tg_ai_bot_dev
DATABASE_PASSWORD=tg_ai_bot_dev

start() {
    if podman container exists "$CONTAINER_NAME"; then
        podman start "$CONTAINER_NAME" >/dev/null
    else
        podman run --detach \
            --name "$CONTAINER_NAME" \
            --publish 127.0.0.1:5433:5432 \
            --volume "$VOLUME_NAME:/var/lib/postgresql/data" \
            --env POSTGRES_DB="$DEV_DATABASE" \
            --env POSTGRES_USER="$DATABASE_USER" \
            --env POSTGRES_PASSWORD="$DATABASE_PASSWORD" \
            "$IMAGE" >/dev/null
    fi

    attempt=1
    while ! podman exec "$CONTAINER_NAME" pg_isready -U "$DATABASE_USER" -d "$DEV_DATABASE" >/dev/null; do
        if [ "$attempt" -ge 30 ]; then
            echo "local development PostgreSQL did not become ready" >&2
            exit 1
        fi
        attempt=$((attempt + 1))
        sleep 1
    done

    echo "Local development PostgreSQL is ready on 127.0.0.1:5433"
}

reset_test_database() {
    start
    podman exec "$CONTAINER_NAME" psql \
        --username "$DATABASE_USER" \
        --dbname postgres \
        --set ON_ERROR_STOP=1 \
        --command "drop database if exists $TEST_DATABASE with (force)"
    podman exec "$CONTAINER_NAME" createdb \
        --username "$DATABASE_USER" \
        "$TEST_DATABASE"
    echo "Local test database $TEST_DATABASE was recreated"
}

case "${1:-start}" in
    start)
        start
        ;;
    stop)
        podman stop "$CONTAINER_NAME"
        ;;
    status)
        podman ps --filter "name=$CONTAINER_NAME"
        ;;
    reset)
        podman rm --force "$CONTAINER_NAME" 2>/dev/null || true
        podman volume rm "$VOLUME_NAME" 2>/dev/null || true
        start
        ;;
    reset-test)
        reset_test_database
        ;;
    *)
        echo "Usage: $0 {start|stop|status|reset|reset-test}" >&2
        exit 2
        ;;
esac
