#!/bin/sh
set -eu

BIN_DIR="${QWE_BIN_DIR:-/qwe-bin}"
IMAGE="${QWE_CONTAINER_IMAGE:-unknown}"

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

contains() {
    case "$1" in
        *"$2"*) return 0 ;;
        *) return 1 ;;
    esac
}

work_dir="$(mktemp -d /tmp/qwe-container-smoke.XXXXXX)"
socket="$work_dir/qwe.sock"
store="$work_dir/qwe.sqlite"
qwed_pid=""

cleanup() {
    if [ -n "$qwed_pid" ] && kill -0 "$qwed_pid" 2>/dev/null; then
        kill -KILL "$qwed_pid" 2>/dev/null || true
        wait "$qwed_pid" 2>/dev/null || true
    fi
    rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

start_qwed() {
    "$BIN_DIR/qwed" --socket "$socket" --store "$store" \
        >"$work_dir/qwed.log" 2>&1 &
    qwed_pid=$!
    attempts=0
    until "$BIN_DIR/qwe" --socket "$socket" ping >/dev/null 2>&1; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 100 ]; then
            cat "$work_dir/qwed.log" >&2
            fail "qwed did not become ready"
        fi
        if ! kill -0 "$qwed_pid" 2>/dev/null; then
            cat "$work_dir/qwed.log" >&2
            fail "qwed exited during startup"
        fi
        sleep 0.05
    done
}

wait_for_snapshot() {
    marker="$1"
    attempts=0
    while :; do
        snapshot="$(
            "$BIN_DIR/qwe" --socket "$socket" session snapshot smoke
        )"
        if contains "$snapshot" "$marker"; then
            printf '%s' "$snapshot"
            return 0
        fi
        attempts=$((attempts + 1))
        [ "$attempts" -lt 100 ] || fail "snapshot did not contain $marker"
        sleep 0.05
    done
}

version="$("$BIN_DIR/qwe" --version)"
start_qwed

ping="$("$BIN_DIR/qwe" --socket "$socket" ping)"
contains "$ping" '"daemon":"ok"' || fail "ping failed: $ping"

created="$(
    "$BIN_DIR/qwe" --socket "$socket" session create smoke \
        sh -- -c 'printf "QWE_CONTAINER_READY\n"; sleep 30'
)"
contains "$created" '"session_id":"smoke"' ||
    fail "session create failed: $created"
contains "$created" '"running":true' ||
    fail "session did not start: $created"

snapshot="$(wait_for_snapshot QWE_CONTAINER_READY)"
contains "$snapshot" '"state":"running"' ||
    fail "Session state is not running: $snapshot"

enter="$(printf '\r')"
input_response="$(
    "$BIN_DIR/qwe" --socket "$socket" session input smoke \
        "printf 'QWE_CONTAINER_INPUT_OK\\n'$enter"
)"
contains "$input_response" '"session_id":"smoke"' ||
    fail "input failed: $input_response"
wait_for_snapshot QWE_CONTAINER_INPUT_OK >/dev/null

calculation='qwe_echo() { echo QWE_FUNCTION=42; }; qwe_echo'
function_response="$(
    "$BIN_DIR/qwe" --socket "$socket" session input smoke \
        "$calculation$enter"
)"
contains "$function_response" '"session_id":"smoke"' ||
    fail "function input failed: $function_response"
snapshot="$(wait_for_snapshot QWE_FUNCTION=42)"

status_before="$("$BIN_DIR/qwe" --socket "$socket" session status smoke)"
pid_before="$(
    printf '%s' "$status_before" |
        sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p'
)"
[ -n "$pid_before" ] || fail "could not read Session PID: $status_before"

tasks_before="$("$BIN_DIR/qwe" --socket "$socket" task history)"
contains "$tasks_before" '"session_id":"smoke"' ||
    fail "Task history is missing the smoke Session: $tasks_before"

kill -KILL "$qwed_pid"
wait "$qwed_pid" 2>/dev/null || true
qwed_pid=""
start_qwed

status_after="$("$BIN_DIR/qwe" --socket "$socket" session status smoke)"
contains "$status_after" '"state":"running"' ||
    fail "Session did not survive supervisor restart: $status_after"
pid_after="$(
    printf '%s' "$status_after" |
        sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p'
)"
[ "$pid_before" = "$pid_after" ] ||
    fail "Session PID changed across restart: $pid_before -> $pid_after"

tasks_after="$("$BIN_DIR/qwe" --socket "$socket" task history)"
contains "$tasks_after" '"session_id":"smoke"' ||
    fail "Task history was not preserved: $tasks_after"

stopped="$("$BIN_DIR/qwe" --socket "$socket" kill-server --confirm)"
contains "$stopped" '"daemon":"stopping"' ||
    fail "kill-server failed: $stopped"
contains "$stopped" '"failures":[]' ||
    fail "kill-server reported teardown failures: $stopped"

attempts=0
while kill -0 "$qwed_pid" 2>/dev/null; do
    attempts=$((attempts + 1))
    [ "$attempts" -lt 100 ] || fail "qwed did not exit after kill-server"
    sleep 0.05
done
qwed_pid=""

printf '%s\n' \
    "{\"case\":\"CONTAINER-001\",\"image\":\"$IMAGE\",\"qwe_version\":\"$version\",\"session_create\":\"pass\",\"input\":\"pass\",\"shell_function\":\"pass\",\"snapshot\":\"pass\",\"task_history\":\"pass\",\"supervisor_restart_adoption\":\"pass\",\"kill_server\":\"pass\"}"
