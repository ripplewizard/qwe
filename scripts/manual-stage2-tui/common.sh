#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)"
STATE_ROOT="${QWE_MANUAL_STATE_ROOT:-/tmp/qwe-stage2-manual-$(id -u)}"
STATE_FILE="$STATE_ROOT/state.env"

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

load_state() {
  [[ -f "$STATE_FILE" ]] || die "review environment not started; run $SCRIPT_DIR/start-review.sh"
  # shellcheck disable=SC1090
  source "$STATE_FILE"
  [[ -n "${QWE_MANUAL_SOCKET:-}" ]] || die "QWE_MANUAL_SOCKET is empty"
  [[ -S "$QWE_MANUAL_SOCKET" ]] || die "qwed socket does not exist: $QWE_MANUAL_SOCKET"
  [[ -x "$QWE_MANUAL_QWE_BIN" ]] || die "qwe binary is missing: $QWE_MANUAL_QWE_BIN"
}

qwe_command() {
  "$QWE_MANUAL_QWE_BIN" \
    --socket "$QWE_MANUAL_SOCKET" \
    --owner "${QWE_MANUAL_OWNER:-manual-reviewer}" \
    "$@"
}

ensure_session() {
  local session_id="$1"
  local program="$2"
  shift 2

  if qwe_command session status "$session_id" >/dev/null 2>&1; then
    printf 'Reusing existing Session: %s\n' "$session_id"
    return
  fi

  if (($#)); then
    qwe_command session create "$session_id" "$program" -- "$@"
  else
    qwe_command session create "$session_id" "$program"
  fi
}

attach_session() {
  local session_id="$1"
  exec "$QWE_MANUAL_QWE_BIN" \
    --socket "$QWE_MANUAL_SOCKET" \
    --owner "${QWE_MANUAL_OWNER:-manual-reviewer}" \
    session attach "$session_id"
}
