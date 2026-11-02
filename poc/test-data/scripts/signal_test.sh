#!/usr/bin/env bash
set -euo pipefail

child_pid=""

cleanup() {
  if [ -n "$child_pid" ]; then
    kill "$child_pid" 2>/dev/null || true
  fi
}

trap cleanup EXIT
trap 'printf "QWE-SIGNAL name=SIGINT pid=%s\n" "$$" >&2; exit 130' INT
trap 'printf "QWE-SIGNAL name=SIGTERM pid=%s\n" "$$" >&2; exit 143' TERM

sleep 300 &
child_pid=$!
printf 'QWE-READY pid=%s child=%s\n' "$$" "$child_pid" >&2
wait "$child_pid"

