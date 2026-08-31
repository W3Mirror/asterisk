#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if (( $# == 0 )); then
  set -- 1000 5000 10000
fi

cargo build -p load-smoke --locked
for concurrent_calls in "$@"; do
  if [[ ! "$concurrent_calls" =~ ^[1-9][0-9]*$ ]]; then
    echo "invalid signaling capacity tier: $concurrent_calls" >&2
    exit 2
  fi
  target/debug/load-smoke "$concurrent_calls" "$concurrent_calls"
done
