#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
STATE_ROOT="${QWE_MANUAL_STATE_ROOT:-/tmp/qwe-stage2-manual-$(id -u)}"
STATE_FILE="$STATE_ROOT/state.env"

if [[ ! -f "$STATE_FILE" ]]; then
  printf 'No review state found at %s\n' "$STATE_FILE"
  exit 0
fi

# shellcheck disable=SC1090
source "$STATE_FILE"

cleanup_owner="manual-reviewer-cleanup"
session_ids=()
shopt -s nullglob
for mapping in "$STATE_ROOT"/*.session; do
  session_ids+=("$(<"$mapping")")
done
shopt -u nullglob
session_ids+=(manual-t03 manual-t04 manual-t08 manual-t11)

for session_id in "${session_ids[@]}"; do
  [[ -n "$session_id" ]] || continue
  if ! "$QWE_MANUAL_QWE_BIN" --socket "$QWE_MANUAL_SOCKET" \
      session status "$session_id" >/dev/null 2>&1; then
    continue
  fi

  lease="$("$QWE_MANUAL_QWE_BIN" --socket "$QWE_MANUAL_SOCKET" \
    lease claim "$session_id" "$cleanup_owner" --ttl-ms 60000 2>/dev/null || true)"
  generation="$(printf '%s' "$lease" | jq -r '.generation // empty' 2>/dev/null || true)"
  if [[ -n "$generation" ]]; then
    "$QWE_MANUAL_QWE_BIN" --socket "$QWE_MANUAL_SOCKET" \
      --owner "$cleanup_owner" session close "$session_id" \
      --generation "$generation" >/dev/null 2>&1 || true
  fi
done

if [[ -n "${QWE_MANUAL_QWED_PID:-}" ]]; then
  kill -TERM "$QWE_MANUAL_QWED_PID" 2>/dev/null || true
  wait "$QWE_MANUAL_QWED_PID" 2>/dev/null || true
fi

pkill -TERM -f -- "--worker-socket ${QWE_MANUAL_SOCKET}.runtime/" 2>/dev/null || true

shopt -s nullglob
for mapping in "$STATE_ROOT"/*.session; do
  rm -f "$mapping"
done
shopt -u nullglob
rm -f "$STATE_FILE"
printf 'Review environment stopped. Logs remain in %s\n' "$STATE_ROOT"
