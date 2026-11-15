#!/usr/bin/env python3
"""Opt-in read-only extraction from two separately configured Telegram DBs."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import stat
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
IDENTIFIER = re.compile(r"^[A-Za-z0-9_.-]+$")
REMOTE_PATH = re.compile(r"^/[A-Za-z0-9_./-]+$")

MESSAGE_SQL = r"""BEGIN READ ONLY;
COPY (
  SELECT encode(convert_to(jsonb_build_object(
    'message_id', m.message_id,
    'created_at', m.created_at,
    'text', m.raw_json -> 'text',
    'caption', m.raw_json -> 'caption',
    'entities', coalesce(m.raw_json -> 'entities', '[]'::jsonb),
    'caption_entities', coalesce(m.raw_json -> 'caption_entities', '[]'::jsonb),
    'rich_message', m.raw_json -> 'rich_message'
  )::text, 'UTF8'), 'hex')
  FROM public.telegram_messages AS m
  JOIN mcp_private.instance_scope AS s
    ON s.scope_key = true
   AND s.instance_id = {instance_literal}
   AND m.chat_id = s.discussion_chat_id
  WHERE m.has_links
  ORDER BY m.created_at, m.message_id
) TO STDOUT;
COMMIT;"""

QR_SQL = r"""BEGIN READ ONLY;
COPY (
  SELECT encode(convert_to(jsonb_build_object(
    'message_id', e.message_id,
    'created_at', e.created_at,
    'target_sha256', e.target_sha256::text,
    'host', e.host,
    'display_hint', e.display_hint,
    'privacy_sensitive', e.privacy_sensitive
  )::text, 'UTF8'), 'hex')
  FROM public.telegram_message_url_evidence AS e
  JOIN mcp_private.instance_scope AS s
    ON s.scope_key = true
   AND s.instance_id = {instance_literal}
   AND e.chat_id = s.discussion_chat_id
  WHERE e.source = 'qr' AND e.kind = 'url'
  ORDER BY e.created_at, e.message_id, e.item_index
) TO STDOUT;
COMMIT;"""


def remote_script(env_file: str, instance: str, sql: str) -> bytes:
    if not REMOTE_PATH.fullmatch(env_file) or ".." in Path(env_file).parts:
        raise ValueError("remote environment file must be an absolute simple path")
    sql = sql.format(instance_literal="'" + instance + "'")
    return (
        "set -eu\n"
        "set -a\n"
        f". {shlex.quote(env_file)}\n"
        "set +a\n"
        "test -n \"${DATABASE_URL:-}\"\n"
        "export PGOPTIONS='-c default_transaction_read_only=on'\n"
        f"psql \"$DATABASE_URL\" -X -qAt -v ON_ERROR_STOP=1 -c {shlex.quote(sql)}\n"
    ).encode()


def run_ssh(target: str, script: bytes, output: Path | None = None) -> bytes:
    if not IDENTIFIER.fullmatch(target):
        raise ValueError("SSH target must be a configured alias")
    command = ["ssh", "-T", "-o", "BatchMode=yes", target, "bash", "-s"]
    if output is None:
        result = subprocess.run(command, input=script, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
        if result.returncode:
            raise RuntimeError(f"read-only database preflight failed (ssh exit {result.returncode})")
        return result.stdout
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as stream:
            result = subprocess.run(command, input=script, stdout=stream, stderr=subprocess.PIPE, check=False)
            stream.flush()
            os.fsync(stream.fileno())
        if result.returncode:
            output.unlink(missing_ok=True)
            raise RuntimeError(f"read-only database export failed (ssh exit {result.returncode})")
    except BaseException:
        output.unlink(missing_ok=True)
        raise
    return b""


def preflight(target: str, env_file: str, instance: str) -> dict[str, object]:
    sql = (
        "BEGIN READ ONLY; SELECT json_build_object("
        "'database', current_database(), 'instance_id', s.instance_id, "
        "'scope_rows', (SELECT count(*) FROM mcp_private.instance_scope), "
        "'link_messages', (SELECT count(*) FROM public.telegram_messages m "
        "WHERE m.chat_id=s.discussion_chat_id AND m.has_links), "
        "'qr_table_present', to_regclass('public.telegram_message_url_evidence') IS NOT NULL) "
        "FROM mcp_private.instance_scope s WHERE s.scope_key=true; ROLLBACK;"
    )
    result = run_ssh(target, remote_script(env_file, instance, sql))
    try:
        rows = [json.loads(line) for line in result.decode().splitlines() if line.strip()]
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError("database preflight returned malformed metadata") from error
    if len(rows) != 1 or rows[0].get("instance_id") != instance or rows[0].get("scope_rows") != 1:
        raise RuntimeError("database scope does not match the requested instance")
    return rows[0]


def count_lines(path: Path) -> int:
    with path.open("rb") as stream:
        return sum(1 for line in stream if line.strip())


def decode_export(encoded_path: Path, decoded_path: Path) -> None:
    fd = os.open(decoded_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with encoded_path.open("rb") as source, os.fdopen(fd, "wb") as destination:
            for line_number, line in enumerate(source, 1):
                if not line.strip():
                    continue
                try:
                    raw = bytes.fromhex(line.decode("ascii").strip())
                    json.loads(raw)
                except Exception as error:  # Do not echo source bytes.
                    raise RuntimeError(f"database export line {line_number} is malformed") from error
                destination.write(raw)
                destination.write(b"\n")
            destination.flush()
            os.fsync(destination.fileno())
    except BaseException:
        decoded_path.unlink(missing_ok=True)
        raise
    finally:
        encoded_path.unlink(missing_ok=True)


def ensure_private_dir(path: Path) -> Path:
    resolved = path.expanduser().resolve()
    try:
        resolved.relative_to(REPO_ROOT)
    except ValueError:
        pass
    else:
        raise ValueError("private output must be outside the repository")
    resolved.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(resolved, 0o700)
    metadata = resolved.stat()
    if stat.S_IMODE(metadata.st_mode) != 0o700 or metadata.st_uid != os.geteuid():
        raise ValueError("private output directory must be owned by the current user with mode 0700")
    return resolved


def parse_source(value: str) -> tuple[str, str]:
    target, separator, path = value.partition(":")
    if not separator or not IDENTIFIER.fullmatch(target) or not REMOTE_PATH.fullmatch(path) or ".." in Path(path).parts:
        raise argparse.ArgumentTypeError("source must be SSH_ALIAS:/absolute/path/to/.env")
    return target, path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--authorized-read-only", action="store_true", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--nedonews-source", type=parse_source, default=parse_source("vps-153:/opt/tg-ai-bot-teloxide/.env"))
    parser.add_argument("--pvo-source", type=parse_source, default=parse_source("vps-153:/opt/nedobot-pvo/.env"))
    args = parser.parse_args()
    if os.environ.get("NEDOBOT_PRIVATE_CORPUS_APPROVED") != "YES":
        parser.error("NEDOBOT_PRIVATE_CORPUS_APPROVED=YES is required after explicit authorization")
    try:
        output_dir = ensure_private_dir(args.output_dir)
        metadata: dict[str, dict[str, object]] = {}
        for instance, source in (("nedonews", args.nedonews_source), ("pvo", args.pvo_source)):
            target, env_file = source
            preflight_data = preflight(target, env_file, instance)
            metadata[instance] = {
                "database": preflight_data["database"],
                "link_messages": preflight_data["link_messages"],
                "qr_evidence_table_present": preflight_data["qr_table_present"],
            }
            raw_path = output_dir / f"{instance}.raw.jsonl"
            qr_path = output_dir / f"{instance}.qr.jsonl"
            raw_encoded = output_dir / f"{instance}.raw.jsonl.b64"
            run_ssh(target, remote_script(env_file, instance, MESSAGE_SQL), raw_encoded)
            decode_export(raw_encoded, raw_path)
            if preflight_data["qr_table_present"]:
                qr_encoded = output_dir / f"{instance}.qr.jsonl.b64"
                run_ssh(target, remote_script(env_file, instance, QR_SQL), qr_encoded)
                decode_export(qr_encoded, qr_path)
            else:
                fd = os.open(qr_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                os.close(fd)
            metadata[instance]["exported_link_messages"] = count_lines(raw_path)
            metadata[instance]["exported_qr_rows"] = count_lines(qr_path)

        metadata_path = output_dir / "source_metadata.json"
        fd = os.open(metadata_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(metadata, stream, sort_keys=True)
            stream.write("\n")
        subprocess.run(
            [
                "cargo", "run", "--quiet", "--locked", "--bin", "export_private_url_history", "--",
                "--authorized-read-only", "--output-dir", str(output_dir),
                "--nedonews-input", str(output_dir / "nedonews.raw.jsonl"),
                "--nedonews-qr", str(output_dir / "nedonews.qr.jsonl"),
                "--pvo-input", str(output_dir / "pvo.raw.jsonl"),
                "--pvo-qr", str(output_dir / "pvo.qr.jsonl"),
            ],
            cwd=REPO_ROOT,
            env={**os.environ, "NEDOBOT_PRIVATE_CORPUS_APPROVED": "YES"},
            check=True,
        )
        print(json.dumps({"output_dir": str(output_dir), "instances": metadata}, sort_keys=True))
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"private history extraction failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
