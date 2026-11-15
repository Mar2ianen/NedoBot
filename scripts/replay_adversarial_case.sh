#!/bin/sh
set -eu

seed=${1:-72623859790382856}
case "$seed" in
    ''|*[!0-9]*)
        echo "seed must be an unsigned decimal integer" >&2
        exit 2
        ;;
esac

export NEDOBOT_PROPTEST_SEED="$seed"
cargo test --test adversarial_harness --locked -- --nocapture
