#!/usr/bin/env python3
"""Run cargo test and fail if Cargo succeeds without executing a passing test."""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path


ANSI_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
PASSED_RE = re.compile(r"^test result: ok\.\s+(\d+) passed;", re.MULTILINE)
RESULT_RE = re.compile(
    r"^test result: (?:ok\.|FAILED\.)\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored;",
    re.MULTILINE,
)
TARGET_RE = re.compile(r"^\s*Running (?:unittests )?([^\n]+) \(([^\n]+)\)$", re.MULTILINE)
TEST_OK_RE = re.compile(r"^test (\S+) \.\.\. ok$", re.MULTILINE)


def verify_inventory(output: str, expected_target: str, expected_names: list[str]) -> bool:
    targets = TARGET_RE.findall(output)
    source, _, binary = expected_target.partition(":")
    return (
        len(targets) == 1
        and targets[0][0] == source
        and (not binary or Path(targets[0][1]).name.startswith(binary + "-"))
        and sorted(TEST_OK_RE.findall(output)) == sorted(expected_names)
    )


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
    results = [(int(passed), int(failed), int(ignored)) for passed, failed, ignored in RESULT_RE.findall(plain_output)]
    if results:
        passed = sum(counts[0] for counts in results)
        failed = sum(counts[1] for counts in results)
        ignored = sum(counts[2] for counts in results)
    else:
        passed = sum(int(count) for count in PASSED_RE.findall(plain_output))
        failed = ignored = 0
    if passed == 0:
        print("error: cargo test exited successfully but ran no passing tests", file=sys.stderr)
        return 1
    if expected_target := os.environ.get("QUALITY_EXPECTED_TARGET"):
        expected_names = os.environ["QUALITY_EXPECTED_NAMES"].split()
        if passed != len(expected_names) or not verify_inventory(plain_output, expected_target, expected_names):
            print(f"error: expected {expected_target}: {expected_names}; "
                  f"observed {TARGET_RE.findall(plain_output)}: {TEST_OK_RE.findall(plain_output)}",
                  file=sys.stderr)
            return 1
        if manifest := os.environ.get("QUALITY_EXECUTION_MANIFEST"):
            with Path(manifest).open("a") as file:
                for name in TEST_OK_RE.findall(plain_output):
                    print(os.environ["NEDOBOT_TEST_SHA"], os.environ["QUALITY_CASE_ID"],
                          os.environ["QUALITY_SEED_APPLICABILITY"], expected_target,
                          name, passed, sep="\t", file=file)
    print(
        f"checked test execution: {passed} passed, {failed} failed, {ignored} ignored"
    )
    return 0


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: run_cargo_test_checked.py <command> [args...]", file=sys.stderr)
        raise SystemExit(2)
    raise SystemExit(run_checked(sys.argv[1:]))
