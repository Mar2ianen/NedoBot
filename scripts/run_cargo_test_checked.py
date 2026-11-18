#!/usr/bin/env python3
"""Run cargo test and fail if Cargo succeeds without executing a passing test."""

from __future__ import annotations

import re
import subprocess
import sys


ANSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
PASSED_RE = re.compile(r"^test result: ok\.\s+(\d+) passed;", re.MULTILINE)


def run_checked(command: list[str]) -> int:
    result = subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )
    sys.stdout.write(result.stdout)
    if result.returncode != 0:
        return result.returncode

    plain_output = ANSI_RE.sub("", result.stdout)
    passed = sum(int(count) for count in PASSED_RE.findall(plain_output))
    if passed == 0:
        print("error: cargo test exited successfully but ran no passing tests", file=sys.stderr)
        return 1
    print(f"checked test execution: {passed} passing test(s)")
    return 0


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: run_cargo_test_checked.py <command> [args...]", file=sys.stderr)
        raise SystemExit(2)
    raise SystemExit(run_checked(sys.argv[1:]))
