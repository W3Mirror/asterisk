#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
scenario_root="${repository_root}/tests/rust-sipp"
image="asterisk-rust-sipp:3.7.2"
server="${repository_root}/target/debug/examples/sipp_uas"
port="${SIPP_PORT:-15060}"
client_port="$((port + 1))"

docker build --tag "${image}" "${scenario_root}"
cargo build --package call-runtime --example sipp_uas --locked

server_pid=""
server_log=""

cleanup() {
  if [[ -n "${server_pid}" ]] && kill -0 "${server_pid}" 2>/dev/null; then
    kill "${server_pid}" 2>/dev/null || true
    wait "${server_pid}" 2>/dev/null || true
  fi
  if [[ -n "${server_log}" ]]; then
    rm -f "${server_log}"
  fi
}
trap cleanup EXIT

run_scenario() {
  local outcome="$1"
  local scenario="$2"
  server_log="$(mktemp)"

  timeout 15s "${server}" "127.0.0.1:${port}" "${outcome}" >"${server_log}" 2>&1 &
  server_pid="$!"

  for _ in {1..100}; do
    if grep -q "^READY " "${server_log}"; then
      break
    fi
    if ! kill -0 "${server_pid}" 2>/dev/null; then
      break
    fi
    sleep 0.05
  done

  if ! grep -q "^READY " "${server_log}"; then
    printf 'Rust SIPp fixture did not become ready for %s\n' "${outcome}" >&2
    sed -n '1,120p' "${server_log}" >&2
    return 1
  fi

  docker run --rm --network host \
    --volume "${scenario_root}:/scenarios:ro" \
    "${image}" "127.0.0.1:${port}" \
    -sf "/scenarios/${scenario}" \
    -s rust-fixture \
    -p "${client_port}" \
    -m 1 \
    -l 1 \
    -nostdin \
    -timeout 10s \
    -trace_err

  wait "${server_pid}"
  server_pid=""
  if ! grep -Eq "^COMPLETE .* ${outcome^} call_" "${server_log}"; then
    printf 'Rust SIPp fixture did not complete cleanly for %s\n' "${outcome}" >&2
    sed -n '1,120p' "${server_log}" >&2
    return 1
  fi

  printf 'SIPp scenario passed: %s\n' "${outcome}"
  rm -f "${server_log}"
  server_log=""
}

run_scenario success success.xml
run_scenario busy busy.xml
run_scenario cancel cancel.xml
