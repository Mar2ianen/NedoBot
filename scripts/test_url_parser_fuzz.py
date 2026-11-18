#!/usr/bin/env python3
"""Generate deterministic URL candidates and run the Rust parser/redactor corpus test."""

from __future__ import annotations

import argparse
import os
import random
import string
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ALPHABET = string.ascii_lowercase + string.digits


def random_token(rng: random.Random, length: int) -> str:
    return "".join(rng.choice(ALPHABET) for _ in range(length))


def generated_urls(count: int, seed: int) -> list[str]:
    rng = random.Random(seed)
    suffixes = ["example", "example.org", "example.co.uk", "пример.рф"]
    candidates: set[str] = set()
    for index in range(count):
        label = random_token(rng, rng.randint(5, 18))
        secret = random_token(rng, rng.randint(8, 32))
        suffix = suffixes[index % len(suffixes)]
        mode = index % 8
        if mode == 0:
            candidates.add(f"https://{label}.{suffix}/r/{secret}?token={secret}#fragment")
        elif mode == 1:
            candidates.add(f"http://{label}.{suffix}/jobs/{secret}?ref={secret}")
        elif mode == 2:
            candidates.add(f"https://t.me/{label}_bot?start={secret}")
        elif mode == 3:
            candidates.add(f"tg://resolve?domain={label}_bot&start={secret}")
        elif mode == 4:
            candidates.add(f"https://{label}.пример.рф/{secret}")
        elif mode == 5:
            candidates.add(f"https://{label}.{suffix}/login/{secret}/continue?code={secret}")
        elif mode == 6:
            candidates.add(f"https://{label}.{suffix}/%2F{secret}?a=b%26c")
        else:
            candidates.add(f"https://t.me/+{random_token(rng, 24)}")
    return sorted(candidates)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--count", type=int, default=10_000, help="number of candidates (default: 10000)")
    parser.add_argument("--seed", type=int, default=20261115, help="deterministic PRNG seed")
    args = parser.parse_args()
    if not 1 <= args.count <= 1_000_000:
        parser.error("--count must be between 1 and 1000000")

    urls = generated_urls(args.count, args.seed)
    fd, path = tempfile.mkstemp(prefix="nedobot-url-fuzz-", suffix=".txt")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as corpus:
            corpus.write("\n".join(urls))
            corpus.write("\n")
        environment = os.environ.copy()
        environment["NEDOBOT_URL_CORPUS"] = path
        command = [
            "cargo",
            "test",
            "--bin",
            "export_url_inventory",
            "tests::extracted_public_chat_url_corpus_is_safe_to_parse_and_redact",
            "--",
            "--ignored",
            "--exact",
            "--nocapture",
        ]
        print(f"Testing {len(urls)} generated candidates with seed {args.seed}...", flush=True)
        return subprocess.run(command, cwd=ROOT, env=environment, check=False).returncode
    finally:
        Path(path).unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
