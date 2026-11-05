#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"
load_state

printf 'Socket: %s\n' "$QWE_MANUAL_SOCKET"
"$QWE_MANUAL_QWE_BIN" --socket "$QWE_MANUAL_SOCKET" ping

for session_id in manual-t03 manual-t04 manual-t08 manual-t11; do
  printf '\n%s\n' "$session_id"
  qwe_command session status "$session_id" || true
done

printf '\nqwed log: %s/qwed.log\n' "$STATE_ROOT"
