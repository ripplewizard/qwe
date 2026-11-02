#!/usr/bin/env bash
set -euo pipefail

duration="${1:-10}"
workers="${2:-1}"
worker_pids=""

cleanup() {
  if [ -n "$worker_pids" ]; then
    for pid in $worker_pids; do
      kill "$pid" 2>/dev/null || true
    done
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

worker=0
while [ "$worker" -lt "$workers" ]; do
  yes > /dev/null &
  worker_pids="$worker_pids $!"
  worker=$((worker + 1))
done

printf 'QWE-CPU-READY workers=%s duration=%s\n' "$workers" "$duration" >&2
sleep "$duration"

