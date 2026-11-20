#!/usr/bin/env python3
"""Emit PG coverage for the added Rust lines; distinguish unmapped from missed."""

import re
import subprocess
import sys
from pathlib import Path


def main(base: str, lcov: str) -> None:
    root = Path.cwd()
    hits: dict[tuple[str, int], int] = {}
    source = None
    for line in Path(lcov).read_text().splitlines():
        if line.startswith("SF:"):
            path = Path(line[3:])
            source = str(path.relative_to(root)) if path.is_relative_to(root) else str(path)
        elif source is not None and line.startswith("DA:"):
            number, count, *_ = line[3:].split(",")
            key = (source, int(number))
            hits[key] = hits.get(key, 0) + int(count)
    diff = subprocess.check_output(
        ["git", "diff", "--no-color", "--no-ext-diff", "--no-renames", "--unified=0", base, "HEAD", "--", "*.rs"],
        text=True,
    )
    print("file\tline\tpg_hits\tstate")
    source = None
    for line in diff.splitlines():
        if line.startswith("+++ "):
            source = line.removeprefix("+++ b/") if line.startswith("+++ b/") else None
        elif source is not None and (match := re.match(r"@@ .* \+(\d+)(?:,(\d+))? @@", line)):
            first, size = int(match[1]), int(match[2] or "1")
            for number in range(first, first + size):
                count = hits.get((source, number))
                state = "unmapped" if count is None else "covered" if count else "missed"
                print(source, number, "" if count is None else count, state, sep="\t")


if __name__ == "__main__":
    main(*sys.argv[1:])
