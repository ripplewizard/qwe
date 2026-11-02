#!/usr/bin/env bash
set -euo pipefail

daemon_pid="${1:-}"

if [ -z "$daemon_pid" ] || ! kill -0 "$daemon_pid" 2>/dev/null; then
  printf 'usage: %s <live-daemon-pid>\n' "$0" >&2
  exit 2
fi

kill -KILL "$daemon_pid"
printf 'QWE-DAEMON-CRASH pid=%s signal=KILL\n' "$daemon_pid" >&2

