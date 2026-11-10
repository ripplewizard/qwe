#!/usr/bin/env python3

import argparse
import json
import os
import signal
import socket
import struct
import subprocess
import tempfile
import time
from pathlib import Path


PROTOCOL_VERSION = 1
MAX_FRAME_BYTES = 8 * 1024 * 1024


def receive_exact(stream, count):
    chunks = []
    remaining = count
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise EOFError("control socket closed during a frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def send_control(path, operation, payload, timeout=10):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"stage7-{time.time_ns()}",
        "idempotency_key": f"{operation.lower()}-{time.time_ns()}",
        "operation": operation,
        "payload": payload,
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(timeout)
        stream.connect(path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        length = struct.unpack(">I", receive_exact(stream, 4))[0]
        assert length <= MAX_FRAME_BYTES
        response = json.loads(receive_exact(stream, length))
    result = response["result"]
    return result.get("Ok", result.get("Err"))


def request(path, operation, payload):
    return send_control(path, operation, payload)


def expect_error(path, operation, payload, code):
    result = request(path, operation, payload)
    assert result.get("code") == code, result


def wait_for_socket(path, process):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            log = Path(getattr(process, "qwe_log")).read_text(errors="replace")
            raise RuntimeError(f"qwed exited with {process.returncode}: {log}")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(path)
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for {path}")


def start_qwed(binary, socket_path, log_path):
    log = open(log_path, "ab", buffering=0)
    process = subprocess.Popen(
        [binary, "--socket", socket_path],
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    process.qwe_log = str(log_path)
    wait_for_socket(socket_path, process)
    return process


def crash_qwed(process):
    if process is None or process.poll() is not None:
        return
    os.kill(process.pid, signal.SIGKILL)
    process.wait(timeout=5)


def stop_qwed(process, socket_path):
    if process is None or process.poll() is not None:
        return
    try:
        send_control(socket_path, "SERVER_KILL", {"confirm": True}, timeout=2)
    except OSError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def wait_for(predicate, message, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise RuntimeError(message)


def claim(path, session_id, owner):
    lease = request(
        path,
        "LEASE_CLAIM",
        {"session_id": session_id, "owner": owner, "ttl_ms": 60000},
    )
    return lease["generation"]


def cgroup_path(pid):
    for line in Path(f"/proc/{pid}/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            return Path("/sys/fs/cgroup") / line[3:].lstrip("/")
    raise RuntimeError(f"PID {pid} has no cgroup v2 path")


def process_gone(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


def wait_snapshot(path, session_id, marker):
    def predicate():
        snapshot = request(path, "SESSION_ATTACH", {"session_id": session_id})
        return snapshot if marker in snapshot.get("screen", "") else None

    return wait_for(predicate, f"snapshot did not contain {marker}")


def probe_profiles(path, session_id):
    status = request(path, "CONTROL_STATUS", {})
    resources = status["capabilities"]["resources"]
    profiles = resources["profiles"]
    assert [profile["name"] for profile in profiles] == [
        "interactive",
        "batch",
        "tight",
        "unlimited",
    ], profiles

    expect_error(
        path,
        "SESSION_CREATE",
        {"session_id": "unknown-profile", "resource_profile": "surprise"},
        "unknown_resource_profile",
    )
    expect_error(
        path,
        "SESSION_CREATE",
        {
            "session_id": "unknown-field",
            "resource_policy": {"surprise": 1},
        },
        "unsupported_resource_field",
    )
    expect_error(
        path,
        "SESSION_CREATE",
        {
            "session_id": "invalid-profile",
            "resource_policy": {"tasks_max": 0},
        },
        "invalid_resource_policy",
    )

    short_lived = request(
        path,
        "SESSION_CREATE",
        {
            "session_id": "short-lived",
            "program": "/bin/true",
            "resource_profile": "tight",
        },
    )
    if short_lived["running"]:
        short_lived = wait_for(
            lambda: (
                status
                if (
                    status := request(
                        path,
                        "SESSION_STATUS",
                        {"session_id": "short-lived"},
                    )
                ).get("running")
                is False
                else None
            ),
            "short-lived Pane did not exit",
        )
    assert short_lived["exit_code"] == 0, short_lived
    assert short_lived["requested_resource_policy"]["tasks_max"] == 64, short_lived
    assert short_lived["effective_resource_policy"]["tasks_max"] == 64, short_lived
    short_lease = request(
        path,
        "LEASE_CLAIM",
        {"session_id": "short-lived", "owner": "stage7-owner", "ttl_ms": 60000},
    )
    updated_short = request(
        path,
        "SESSION_RESOURCE_UPDATE",
        {
            "session_id": "short-lived",
            "owner": "stage7-owner",
            "generation": short_lease["generation"],
            "resource_policy": {"tasks_max": 48},
        },
    )
    assert updated_short["resource_policy"]["tasks_max"] == 48, updated_short

    created = request(
        path,
        "SESSION_CREATE",
        {
            "session_id": session_id,
            "program": "sh",
            "args": ["-c", "sleep 60"],
            "resource_profile": "tight",
        },
    )
    assert created["resource_profile"] == "tight", created
    assert created["requested_resource_policy"]["memory_max_bytes"] == 256 * 1024 * 1024
    assert created["requested_resource_policy"]["tasks_max"] == 64
    assert created["effective_resource_policy"]["memory_max_bytes"] == 256 * 1024 * 1024
    assert created["effective_resource_policy"]["tasks_max"] == 64
    assert created["resource_metrics"]["tier"] == "hard", created

    scope = cgroup_path(created["pid"])
    assert scope.name == f"qwe-pane-{created['pane_id']}.scope", scope
    assert scope.is_dir(), scope
    return created


def probe_cli_profile(qwe, path):
    completed = subprocess.run(
        [
            qwe,
            "--socket",
            path,
            "session",
            "create",
            "cli-profile",
            "sh",
            "--profile",
            "tight",
            "--",
            "-c",
            "sleep 30",
        ],
        text=True,
        capture_output=True,
        timeout=10,
    )
    assert completed.returncode == 0, completed.stderr
    created = json.loads(completed.stdout)
    assert created["resource_profile"] == "tight", created
    assert created["requested_resource_policy"]["tasks_max"] == 64, created
    generation = claim(path, "cli-profile", "stage7-owner")
    request(
        path,
        "SESSION_KILL",
        {
            "session_id": "cli-profile",
            "owner": "stage7-owner",
            "generation": generation,
        },
    )


def probe_updates_and_restart(path, session_id, created, owner):
    generation = claim(path, session_id, owner)
    last = None
    for tasks_max in range(24, 64):
        last = request(
            path,
            "SESSION_RESOURCE_UPDATE",
            {
                "session_id": session_id,
                "owner": owner,
                "generation": generation,
                "resource_profile": "tight",
                "resource_policy": {
                    "memory_max_bytes": 128 * 1024 * 1024,
                    "tasks_max": tasks_max,
                },
            },
        )
    assert last["resource_policy"]["tasks_max"] == 63, last

    status = request(path, "SESSION_STATUS", {"session_id": session_id})
    metrics = status["resource_metrics"]
    assert status["resource_profile"] == "custom", status
    assert status["requested_resource_policy"]["memory_max_bytes"] == 128 * 1024 * 1024
    assert 128 * 1024 * 1024 >= metrics["memory_peak_bytes"]
    assert metrics["limits"]["tasks_max"] == 63, metrics
    assert metrics["remaining"]["tasks_max"] is not None, metrics
    assert metrics["memory_events"], metrics
    assert metrics["pids_events"], metrics
    assert metrics["cpu_stat"], metrics
    assert len(metrics["events"]) == 32, metrics
    assert metrics["event_retention"]["maximum_events"] == 32, metrics
    assert request(path, "PING", {})["daemon"] == "ok"
    return generation


def probe_detached_cleanup(path):
    script = (
        "import subprocess,time;"
        "p=subprocess.Popen(['sleep','300'],start_new_session=True);"
        "print(f'DETACHED_PID={p.pid}',flush=True);"
        "time.sleep(300)"
    )
    created = request(
        path,
        "SESSION_CREATE",
        {
            "session_id": "detached-cleanup",
            "program": "python3",
            "args": ["-c", script],
            "resource_profile": "tight",
        },
    )
    snapshot = wait_snapshot(path, "detached-cleanup", "DETACHED_PID=")
    marker = next(
        line for line in snapshot["screen"].splitlines() if line.startswith("DETACHED_PID=")
    )
    detached_pid = int(marker.split("=", 1)[1])
    detached_cgroup = cgroup_path(detached_pid)
    assert detached_cgroup.name == f"qwe-pane-{created['pane_id']}.scope"
    generation = claim(path, "detached-cleanup", "stage7-owner")

    killed = request(
        path,
        "SESSION_KILL",
        {
            "session_id": "detached-cleanup",
            "owner": "stage7-owner",
            "generation": generation,
        },
    )
    assert killed["session_id"] == "detached-cleanup", killed
    wait_for(
        lambda: process_gone(detached_pid),
        f"detached PID {detached_pid} survived Pane teardown",
    )
    wait_for(
        lambda: not detached_cgroup.exists(),
        f"Pane scope {detached_cgroup} survived teardown",
    )
    assert request(path, "PING", {})["daemon"] == "ok"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--qwe", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    qwed = None
    try:
        with tempfile.TemporaryDirectory(prefix="qwe-stage7-") as log_dir:
            log_path = Path(log_dir) / "qwed.log"
            qwed = start_qwed(args.qwed, args.socket, log_path)
            created = probe_profiles(args.socket, "profile-session")
            probe_cli_profile(args.qwe, args.socket)
            generation = probe_updates_and_restart(
                args.socket, "profile-session", created, "stage7-owner"
            )

            crash_qwed(qwed)
            qwed = start_qwed(args.qwed, args.socket, log_path)
            recovered = wait_for(
                lambda: (
                    status
                    if (
                        status := request(
                            args.socket,
                            "SESSION_STATUS",
                            {"session_id": "profile-session"},
                        )
                    ).get("running")
                    else None
                ),
                "Session did not recover after qwed restart",
            )
            assert recovered["pane_id"] == created["pane_id"], recovered
            assert recovered["requested_resource_policy"]["tasks_max"] == 63, recovered
            assert recovered["effective_resource_policy"]["tasks_max"] == 63, recovered

            request(
                args.socket,
                "SESSION_KILL",
                {
                    "session_id": "profile-session",
                    "owner": "stage7-owner",
                    "generation": generation,
                },
            )
            probe_detached_cleanup(args.socket)
            print(
                json.dumps(
                    {
                        "case": "RG-001/RG-002/RG-003/RG-006",
                        "profiles": "pass",
                        "policy_errors": "pass",
                        "short_lived_startup": "pass",
                        "cli_profile": "pass",
                        "requested_effective_policy": "pass",
                        "restart_persistence": "pass",
                        "event_retention": "pass",
                        "detached_scope_cleanup": "pass",
                    },
                    separators=(",", ":"),
                )
            )
    finally:
        stop_qwed(qwed, args.socket)


if __name__ == "__main__":
    main()
