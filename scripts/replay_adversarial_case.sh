#!/bin/sh
set -eu

seed=${1:-72623859790382856}
exec "$(dirname "$0")/replay_quality_case.sh" seeded-url-property "$seed"
