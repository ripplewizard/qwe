#!/usr/bin/env python3

import argparse
import json
import os
import signal
import socket
import struct
import subprocess
import time


PROTOCOL_VERSION = 1
ATTACH_OPEN = 0x01
ATTACH_ACCEPTED = 0x81
SNAPSHOT = 0x82


def receive_exact(stream, count):
    chunks = []
    remaining = count
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise EOFError("socket closed during a frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def receive_frame(stream):
    frame_type, length = struct.unpack(">BI", receive_exact(stream, 5))
    return frame_type, receive_exact(stream, length)


def send_attach(stream, session_id, owner):
    frame = {
        "protocol_version": PROTOCOL_VERSION,
        "session_id": session_id,
        "owner": owner,
        "rows": 24,
        "columns": 80,
        "mode": "exclusive",
    }
    payload = json.dumps(frame, separators=(",", ":")).encode()
    stream.sendall(struct.pack(">BI", ATTACH_OPEN, len(payload)))
    stream.sendall(payload)


def connect_attach(path):
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stream.settimeout(5)
    stream.connect(path)
    return stream


def send_control(path, operation, payload, key=None):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"control-authority-{time.time_ns()}",
        "idempotency_key": key,
        "operation": operation,
        "payload": payload,
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(10)
        stream.connect(path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        length = struct.unpack(">I", receive_exact(stream, 4))[0]
        response = json.loads(receive_exact(stream, length))
    result = response["result"]
    return result.get("Ok", result.get("Err"))


def expect_error(path, operation, payload, code):
    result = send_control(
        path,
        operation,
        payload,
        key=f"expect-{operation.lower()}-{time.time_ns()}",
    )
    assert result.get("code") == code, result
    return result


def wait_for_socket(path, process):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"qwed exited with status {process.returncode}")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(path)
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for {path}")


def start_qwed(binary, socket_path):
    process = subprocess.Popen(
        [binary, "--socket", socket_path],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    wait_for_socket(socket_path, process)
    return process


def stop_qwed(process):
    if process is None or process.poll() is not None:
        return
    os.kill(process.pid, signal.SIGTERM)
    process.wait(timeout=5)


def create_session(path, session_id):
    created = send_control(
        path,
        "SESSION_CREATE",
        {"session_id": session_id, "program": "sh", "args": []},
        key=f"create-{session_id}",
    )
    assert created["running"] is True, created
    return created


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()
    attach_path = f"{args.socket}.attach"
    qwed = None
    worker_pids = []
    active = None
    try:
        qwed = start_qwed(args.qwed, args.socket)

        held_session = "control-held"
        held_created = create_session(args.socket, held_session)
        held_task = held_created["task_id"]
        worker_pids.append(held_created["worker_pid"])
        unattached_status = send_control(args.socket, "CONTROL_STATUS", {})
        assert unattached_status["daemon"] == "ok", unattached_status
        unattached_session = send_control(
            args.socket,
            "SESSION_STATUS",
            {"session_id": held_session},
        )
        assert unattached_session["running"] is True, unattached_session
        expect_error(
            args.socket,
            "TASK_STOP",
            {"task_id": held_task, "owner": "held-owner"},
            "not_attached",
        )
        expect_error(
            args.socket,
            "SESSION_CLOSE",
            {"session_id": held_session, "owner": "held-owner"},
            "not_attached",
        )
        expect_error(
            args.socket,
            "SESSION_INPUT",
            {
                "session_id": held_session,
                "owner": "held-owner",
                "data": "printf 'UNATTACHED_INPUT\\n'\r",
            },
            "not_attached",
        )
        expect_error(
            args.socket,
            "LEASE_TOUCH",
            {
                "session_id": held_session,
                "owner": "held-owner",
                "generation": 1,
                "ttl_ms": 60000,
            },
            "not_attached",
        )
        lease = send_control(
            args.socket,
            "LEASE_CLAIM",
            {"session_id": held_session, "owner": "held-owner", "ttl_ms": 60000},
            key="held-claim",
        )
        generation = lease["generation"]
        assert generation == 1, lease
        held_again = send_control(
            args.socket,
            "LEASE_CLAIM",
            {
                "session_id": held_session,
                "owner": "held-owner",
                "ttl_ms": 60000,
            },
            key="held-reclaim",
        )
        assert held_again["generation"] == generation, held_again
        expect_error(
            args.socket,
            "LEASE_TOUCH",
            {
                "session_id": held_session,
                "owner": "wrong-owner",
                "generation": generation,
                "ttl_ms": 60000,
            },
            "not_owner",
        )
        expect_error(
            args.socket,
            "TASK_STOP",
            {
                "task_id": held_task,
                "owner": "wrong-owner",
                "generation": generation,
            },
            "not_owner",
        )
        expect_error(
            args.socket,
            "TASK_STOP",
            {
                "task_id": held_task,
                "owner": "held-owner",
                "generation": generation + 1,
            },
            "not_owner",
        )
        expect_error(
            args.socket,
            "SESSION_CLOSE",
            {
                "session_id": held_session,
                "owner": "held-owner",
                "generation": generation + 1,
            },
            "not_owner",
        )
        expect_error(
            args.socket,
            "SESSION_INPUT",
            {
                "session_id": held_session,
                "owner": "held-owner",
                "generation": generation,
                "data": "printf 'HELD_INPUT\\n'\r",
            },
            "not_attached",
        )
        stopped = send_control(
            args.socket,
            "TASK_STOP",
            {
                "task_id": held_task,
                "owner": "held-owner",
                "generation": generation,
            },
            key="held-task-stop",
        )
        assert stopped["state"] == "stopped", stopped
        closed = send_control(
            args.socket,
            "SESSION_CLOSE",
            {
                "session_id": held_session,
                "owner": "held-owner",
                "generation": generation,
            },
            key="held-session-close",
        )
        assert closed["state"] == "closed", closed
        released = send_control(
            args.socket,
            "LEASE_STATUS",
            {"session_id": held_session},
        )
        assert released["state"] == "released", released
        assert released["generation"] == generation + 1, released

        active_session = "control-active"
        active_created = create_session(args.socket, active_session)
        active_task = active_created["task_id"]
        worker_pids.append(active_created["worker_pid"])
        owner = "active-owner"
        active = connect_attach(attach_path)
        send_attach(active, active_session, owner)
        frame_type, payload = receive_frame(active)
        assert frame_type == ATTACH_ACCEPTED
        assert json.loads(payload)["owner"] == owner
        frame_type, _ = receive_frame(active)
        assert frame_type == SNAPSHOT

        expect_error(
            args.socket,
            "LEASE_TOUCH",
            {
                "session_id": active_session,
                "owner": owner,
                "generation": 0,
                "ttl_ms": 60000,
            },
            "not_attached",
        )
        expect_error(
            args.socket,
            "LEASE_RELEASE",
            {
                "session_id": active_session,
                "owner": owner,
                "generation": 0,
            },
            "not_attached",
        )
        expect_error(
            args.socket,
            "TASK_STOP",
            {"task_id": active_task, "owner": "wrong-owner"},
            "not_owner",
        )
        expect_error(
            args.socket,
            "SESSION_CLOSE",
            {"session_id": active_session, "owner": "wrong-owner"},
            "not_owner",
        )
        expect_error(
            args.socket,
            "SESSION_INPUT",
            {
                "session_id": active_session,
                "owner": "wrong-owner",
                "data": "printf 'WRONG_OWNER_INPUT\\n'\r",
            },
            "not_owner",
        )
        accepted_input = send_control(
            args.socket,
            "SESSION_INPUT",
            {
                "session_id": active_session,
                "owner": owner,
                "data": "printf 'ACTIVE_INPUT_OK\\n'\r",
            },
            key="active-owner-input",
        )
        assert accepted_input["running"] is True, accepted_input
        closed = send_control(
            args.socket,
            "SESSION_CLOSE",
            {"session_id": active_session, "owner": owner},
            key="active-session-close",
        )
        assert closed["state"] == "closed", closed
        active.close()
        active = None

        stop_session = "control-active-stop"
        stop_created = create_session(args.socket, stop_session)
        stop_task = stop_created["task_id"]
        worker_pids.append(stop_created["worker_pid"])
        active = connect_attach(attach_path)
        send_attach(active, stop_session, owner)
        frame_type, _ = receive_frame(active)
        assert frame_type == ATTACH_ACCEPTED
        frame_type, _ = receive_frame(active)
        assert frame_type == SNAPSHOT
        stopped = send_control(
            args.socket,
            "TASK_STOP",
            {"task_id": stop_task, "owner": owner},
            key="active-task-stop",
        )
        assert stopped["state"] == "stopped", stopped
        while True:
            try:
                receive_frame(active)
            except (EOFError, OSError):
                break
        active.close()
        active = None

        stop_lease = send_control(
            args.socket,
            "LEASE_CLAIM",
            {
                "session_id": stop_session,
                "owner": "stopped-cleanup-owner",
                "ttl_ms": 60000,
            },
            key="active-stop-cleanup-lease",
        )
        stop_closed = send_control(
            args.socket,
            "SESSION_CLOSE",
            {
                "session_id": stop_session,
                "owner": "stopped-cleanup-owner",
                "generation": stop_lease["generation"],
            },
            key="active-stop-cleanup-close",
        )
        assert stop_closed["state"] == "closed", stop_closed

        print(
            json.dumps(
                {
                    "case": "AUTH-009/AUTH-010/AUTH-012/AUTH-017/AUTH-018/AUTH-020",
                    "unattached_rejection": "pass",
                    "held_authorization": "pass",
                    "active_authorization": "pass",
                    "close_release_tombstone": "pass",
                },
                separators=(",", ":"),
            )
        )
    finally:
        if active is not None:
            active.close()
        stop_qwed(qwed)
        for worker_pid in worker_pids:
            try:
                os.kill(worker_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


if __name__ == "__main__":
    main()
