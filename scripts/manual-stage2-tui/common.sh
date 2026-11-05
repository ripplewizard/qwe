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
  local case_id="$1"
  local program="$2"
  shift 2

  local mapping="$STATE_ROOT/$case_id.session"
  if [[ -f "$mapping" ]]; then
    local active_id
    active_id="$(<"$mapping")"
    if [[ -n "$active_id" ]] &&
       qwe_command status |
         jq -e --arg id "$active_id" \
           'any(.sessions[]; .id == $id and .state == "running")' \
           >/dev/null; then
      printf '%s\n' "$active_id"
      return
    fi
    rm -f "$mapping"
  fi

  local session_id
  session_id="$case_id-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  if (($#)); then
    qwe_command session create "$session_id" "$program" -- "$@" >&2
  else
    qwe_command session create "$session_id" "$program" >&2
  fi
  printf '%s\n' "$session_id" >"$mapping"
  printf '%s\n' "$session_id"
}

attach_session() {
  local session_id="$1"
  exec "$QWE_MANUAL_QWE_BIN" \
    --socket "$QWE_MANUAL_SOCKET" \
    --owner "${QWE_MANUAL_OWNER:-manual-reviewer}" \
    session attach "$session_id"
}
