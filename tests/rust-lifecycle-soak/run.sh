#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if (( $# > 7 )); then
  echo "usage: $0 [minimum_seconds] [minimum_cycles] [calls_per_cycle] [packets_per_answered_call] [queue_capacity] [warmup_cycles] [max_resident_drift_bytes]" >&2
  exit 2
fi

minimum_seconds="${1:-7200}"
minimum_cycles="${2:-8}"
calls_per_cycle="${3:-12}"
packets_per_answered_call="${4:-8}"
queue_capacity="${5:-4}"
warmup_cycles="${6:-2}"
max_resident_drift_bytes="${7:-67108864}"

for value in \
  "$minimum_seconds" \
  "$minimum_cycles" \
  "$calls_per_cycle" \
  "$packets_per_answered_call" \
  "$queue_capacity" \
  "$warmup_cycles" \
  "$max_resident_drift_bytes"; do
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "lifecycle soak arguments must be non-negative integers: $value" >&2
    exit 2
  fi
done

if (( minimum_seconds > 86400 )); then
  echo "minimum_seconds exceeds the 24-hour harness safety bound: $minimum_seconds" >&2
  exit 2
fi

cargo build -p load-smoke --release --locked
target/release/load-smoke soak \
  "$minimum_cycles" \
  "$minimum_seconds" \
  "$calls_per_cycle" \
  "$packets_per_answered_call" \
  "$queue_capacity" \
  "$warmup_cycles" \
  "$max_resident_drift_bytes"
