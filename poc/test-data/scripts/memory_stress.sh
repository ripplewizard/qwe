#!/usr/bin/env bash
set -euo pipefail

megabytes="${1:-128}"
duration="${2:-10}"
python_pid=""

cleanup() {
  if [ -n "$python_pid" ]; then
    kill "$python_pid" 2>/dev/null || true
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

python3 - "$megabytes" "$duration" <<'PY' &
import os
import sys
import time

megabytes = int(sys.argv[1])
duration = float(sys.argv[2])
data = bytearray(megabytes * 1024 * 1024)
for offset in range(0, len(data), 4096):
    data[offset] = 1

print(
    f"QWE-MEMORY-READY pid={os.getpid()} bytes={len(data)}",
    file=sys.stderr,
    flush=True,
)
time.sleep(duration)
PY
python_pid=$!
wait "$python_pid"

