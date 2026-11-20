#!/usr/bin/env python3
"""Strict structural/privacy gate for the committed synthetic URL history corpus."""

import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit

CORPUS = Path(__file__).resolve().parents[1] / "testing/corpus/public/synthetic_url_history.jsonl"
ALLOWED_HOSTS = {"example.org", "t.me"}
SECRETISH = re.compile(r"(?i)(?:bot\d{5,}:[A-Za-z0-9_-]{20,}|(?:token|secret|api[_-]?key)=|@[A-Za-z0-9_]{5,})")
NUMERIC_ID = re.compile(r"(?<![A-Za-z])(?:-100)?\d{7,}(?![A-Za-z])")
REQUIRED = {
    "schema_version", "id", "instance", "validity", "observed_at", "class",
    "source", "canonical_url_identity", "label", "label_provenance",
    "evidence_source", "bot_api_version", "teloxide_revision", "cross_field_assumptions",
}


def validate(path: Path = CORPUS) -> int:
    seen = set()
    for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line:
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"line {line_no}: invalid JSON") from exc
        if not isinstance(item, dict) or set(item) - REQUIRED - {"qr_target_sha256", "qr_url_recoverable"} or REQUIRED - set(item):
            raise ValueError(f"line {line_no}: unexpected corpus fields")
        if item["schema_version"] != 1 or item["validity"] != "V2_synthetic":
            raise ValueError(f"line {line_no}: invalid version or validity")
        if item["id"] in seen or not re.fullmatch(r"history-[0-9]{3}", item["id"]):
            raise ValueError(f"line {line_no}: IDs must be unique synthetic IDs")
        seen.add(item["id"])
        if item["instance"] not in {"synthetic_a", "synthetic_b"}:
            raise ValueError(f"line {line_no}: non-synthetic instance")
        if item["label"] is not None or item["label_provenance"] != "synthetic_unlabeled":
            raise ValueError(f"line {line_no}: real label provenance is forbidden")
        url = item["canonical_url_identity"]
        if item["class"] == "qr_hash_only":
            if url is not None or item.get("qr_url_recoverable") is not False or not re.fullmatch(r"[a-f0-9]{64}", item.get("qr_target_sha256", "")):
                raise ValueError(f"line {line_no}: QR hashes must remain unrecoverable")
        else:
            parsed = urlsplit(url or "")
            if parsed.scheme != "https" or parsed.hostname not in ALLOWED_HOSTS or parsed.username or parsed.password:
                raise ValueError(f"line {line_no}: URL host is not synthetic/approved")
        inspected = " ".join(
            str(value) for key, value in item.items()
            if key not in {"qr_target_sha256", "teloxide_revision"}
        )
        if SECRETISH.search(inspected) or NUMERIC_ID.search(inspected):
            raise ValueError(f"line {line_no}: token-like or numeric identity detected")
    if len(seen) < 1:
        raise ValueError("public synthetic corpus is empty")
    return len(seen)


if __name__ == "__main__":
    try:
        print(f"validated {validate()} synthetic records")
    except (OSError, ValueError) as exc:
        print(f"public URL corpus rejected: {exc}", file=sys.stderr)
        raise SystemExit(1)
