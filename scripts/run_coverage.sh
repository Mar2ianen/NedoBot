#!/bin/sh
set -eu

if ! cargo llvm-cov --version >/dev/null 2>&1; then
    echo "install cargo-llvm-cov 0.9.1 before collecting coverage" >&2
    exit 2
fi

export LLVM_COV="${LLVM_COV:-$(command -v llvm-cov)}"
export LLVM_PROFDATA="${LLVM_PROFDATA:-$(command -v llvm-profdata)}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-3}"

exec cargo llvm-cov --workspace --all-targets --locked --summary-only
