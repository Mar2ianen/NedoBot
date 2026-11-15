#!/usr/bin/env python3
"""Export the legacy shared SQLite spam-reputation database to JSON."""

import json
import os
import sqlite3
import sys
import tempfile
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} LEGACY_SQLITE_PATH JSON_LEDGER_PATH", file=sys.stderr)
        return 2

    source = Path(sys.argv[1]).resolve(strict=True)
    destination = Path(sys.argv[2]).resolve()
    if source == destination:
        raise SystemExit("source and destination must be different files")
    destination.parent.mkdir(parents=True, exist_ok=True)

    connection = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    try:
        rows = connection.execute(
            "select instance_id, telegram_user_id, is_spammer "
            "from spammer_decisions order by instance_id, telegram_user_id"
        ).fetchall()
    finally:
        connection.close()

    payload = {
        "version": 1,
        "decisions": [
            {
                "instance_id": instance_id,
                "telegram_user_id": int(user_id),
                "is_spammer": bool(is_spammer),
            }
            for instance_id, user_id, is_spammer in rows
        ],
    }
    fd, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}.", dir=destination.parent)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as temporary:
            json.dump(payload, temporary, ensure_ascii=False, separators=(",", ":"))
            temporary.write("\n")
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary_name, destination)
        dir_fd = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        if os.path.exists(temporary_name):
            os.unlink(temporary_name)

    print(f"exported {len(rows)} decisions to {destination}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
