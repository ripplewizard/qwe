#!/usr/bin/env bash
set -euo pipefail

remaining="${1:-3}"
sleep_seconds="${2:-30}"

printf 'QWE-PROCESS pid=%s depth=%s\n' "$$" "$remaining" >&2

if [ "$remaining" -gt 1 ]; then
  "$0" "$((remaining - 1))" "$sleep_seconds"
fi

if [ "$remaining" -eq 1 ]; then
  sleep "$sleep_seconds"
fi

