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

for command in cargo jq nvim htop; do
  command -v "$command" >/dev/null 2>&1 || die "missing command: $command"
done

if [[ -f "$STATE_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$STATE_FILE"
  if [[ -x "$QWE_MANUAL_QWE_BIN" ]] &&
     "$QWE_MANUAL_QWE_BIN" --socket "$QWE_MANUAL_SOCKET" ping >/dev/null 2>&1; then
    printf 'Review environment is already running.\n'
    printf 'Socket: %s\n' "$QWE_MANUAL_SOCKET"
    printf 'Run %s/status.sh to inspect it.\n' "$SCRIPT_DIR"
    exit 0
  fi
  die "stale review state exists; run $SCRIPT_DIR/stop-review.sh first"
fi

mkdir -p "$STATE_ROOT"
chmod 700 "$STATE_ROOT"

socket="$STATE_ROOT/qwe.sock"
fixture="$STATE_ROOT/qwe-stage2-large.txt"
qwed_log="$STATE_ROOT/qwed.log"

cd "$REPO_ROOT"
cargo build -p qwe -p qwed -p qwe-worker

for index in $(seq 1 200); do
  printf 'QWE_MANUAL_LINE_%s\n' "$index"
done >"$fixture"

: >"$qwed_log"
nohup "$REPO_ROOT/target/debug/qwed" --socket "$socket" \
  >"$qwed_log" 2>&1 &
qwed_pid=$!

for _ in $(seq 1 250); do
  if "$REPO_ROOT/target/debug/qwe" --socket "$socket" ping >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$qwed_pid" 2>/dev/null; then
    sed -n '1,160p' "$qwed_log" >&2 || true
    die "qwed exited during startup"
  fi
  sleep 0.02
done

"$REPO_ROOT/target/debug/qwe" --socket "$socket" ping >/dev/null ||
  die "qwed did not become ready"

{
  printf 'QWE_MANUAL_REPO_ROOT=%q\n' "$REPO_ROOT"
  printf 'QWE_MANUAL_QWE_BIN=%q\n' "$REPO_ROOT/target/debug/qwe"
  printf 'QWE_MANUAL_SOCKET=%q\n' "$socket"
  printf 'QWE_MANUAL_FIXTURE=%q\n' "$fixture"
  printf 'QWE_MANUAL_QWED_PID=%q\n' "$qwed_pid"
  printf 'QWE_MANUAL_OWNER=%q\n' "manual-reviewer"
} >"$STATE_FILE"

printf 'Stage 2 manual review environment started.\n'
printf 'State file: %s\n' "$STATE_FILE"
printf 'Socket: %s\n' "$socket"
printf 'qwed PID: %s\n' "$qwed_pid"
printf 'qwed log: %s\n' "$qwed_log"
printf '\n'
printf 'Run one case at a time:\n'
printf '  %s/t03-nvim.sh\n' "$SCRIPT_DIR"
printf '  %s/t04-htop.sh\n' "$SCRIPT_DIR"
printf '  %s/t08-detach-reattach.sh\n' "$SCRIPT_DIR"
printf '  %s/t11-job-control.sh\n' "$SCRIPT_DIR"
printf '  %s/status.sh\n' "$SCRIPT_DIR"
printf '  %s/stop-review.sh\n' "$SCRIPT_DIR"
