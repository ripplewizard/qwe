#!/usr/bin/env bash
set -euo pipefail

duration="${1:-300}"
child_pid=""

cleanup() {
  if [ -n "$child_pid" ]; then
    kill "$child_pid" 2>/dev/null || true
  fi
}

trap cleanup EXIT
trap 'printf "QWE-TIMEOUT pid=%s\n" "$$" >&2; exit 124' TERM

printf 'QWE-READY pid=%s timeout=%s\n' "$$" "$duration" >&2
sleep "$duration" &
child_pid=$!
wait "$child_pid"

