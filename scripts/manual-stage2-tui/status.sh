#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
load_state

printf 'Socket: %s\n' "$QWE_MANUAL_SOCKET"
"$QWE_MANUAL_QWE_BIN" --socket "$QWE_MANUAL_SOCKET" ping

shopt -s nullglob
for mapping in "$STATE_ROOT"/*.session; do
  case_id="$(basename "$mapping" .session)"
  session_id="$(<"$mapping")"
  printf '\n%s: %s\n' "$case_id" "$session_id"
  qwe_command session status "$session_id" || true
done
shopt -u nullglob

printf '\nqwed log: %s/qwed.log\n' "$STATE_ROOT"
