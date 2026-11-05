#!/usr/bin/env python3

import argparse
import json
import os
import signal
import socket
import struct
import subprocess
import time


ATTACH_OPEN = 0x01
DETACH = 0x05
ATTACH_ACCEPTED = 0x81
SNAPSHOT = 0x82
ERROR = 0x87
PROTOCOL_VERSION = 1


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


def send_json(stream, frame_type, value):
    payload = json.dumps(value, separators=(",", ":")).encode()
    stream.sendall(struct.pack(">BI", frame_type, len(payload)))
    stream.sendall(payload)


def send_control(path, operation, payload, key):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"reclaim-{time.time_ns()}",
        "idempotency_key": key,
        "operation": operation,
        "payload": payload,
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        length = struct.unpack(">I", receive_exact(stream, 4))[0]
        response = json.loads(receive_exact(stream, length))
    result = response["result"]
    return result.get("Ok", result.get("Err"))


def attach_open(session_id, owner, token=None):
    frame = {
        "protocol_version": PROTOCOL_VERSION,
        "session_id": session_id,
        "owner": owner,
        "rows": 24,
        "columns": 80,
        "mode": "exclusive",
    }
    if token is not None:
        frame["reclaim_token"] = token
    return frame


def connect(path):
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stream.settimeout(5)
    stream.connect(path)
    return stream


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


def stop_qwed(process, force=False):
    if process is None or process.poll() is not None:
        return
    os.kill(process.pid, signal.SIGKILL if force else signal.SIGTERM)
    process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()
    attach_path = f"{args.socket}.attach"
    session_id = "reclaim-probe"
    first_qwed = None
    second_qwed = None
    active = None
    worker_pid = None
    try:
        first_qwed = start_qwed(args.qwed, args.socket)
        created = send_control(
            args.socket,
            "SESSION_CREATE",
            {"session_id": session_id, "program": "sh", "args": []},
            "reclaim-create",
        )
        worker_pid = created["worker_pid"]

        active = connect(attach_path)
        send_json(active, ATTACH_OPEN, attach_open(session_id, "reclaim-owner"))
        frame_type, payload = receive_frame(active)
        assert frame_type == ATTACH_ACCEPTED
        first_accepted = json.loads(payload)
        token = first_accepted["reclaim_token"]
        frame_type, _ = receive_frame(active)
        assert frame_type == SNAPSHOT

        stop_qwed(first_qwed, force=True)
        first_qwed = None
        try:
            active.recv(1)
        except (ConnectionResetError, OSError):
            pass
        active.close()
        active = None

        second_qwed = start_qwed(args.qwed, args.socket)
        grace_status = send_control(args.socket, "CONTROL_STATUS", {}, None)
        assert grace_status["daemon"] == "ok", grace_status
        session_status = send_control(
            args.socket,
            "SESSION_STATUS",
            {"session_id": session_id},
            None,
        )
        assert session_status["running"] is True, session_status
        for operation, payload in [
            (
                "SESSION_INPUT",
                {
                    "session_id": session_id,
                    "owner": "reclaim-owner",
                    "data": "printf 'GRACE_INPUT\\n'\r",
                },
            ),
            (
                "TASK_STOP",
                {
                    "task_id": created["task_id"],
                    "owner": "reclaim-owner",
                },
            ),
            (
                "SESSION_CLOSE",
                {
                    "session_id": session_id,
                    "owner": "reclaim-owner",
                },
            ),
            (
                "LEASE_CLAIM",
                {
                    "session_id": session_id,
                    "owner": "grace-claim",
                    "ttl_ms": 60000,
                },
            ),
        ]:
            rejected = send_control(
                args.socket,
                operation,
                payload,
                f"grace-{operation.lower()}",
            )
            assert rejected["code"] == "not_attached", (operation, rejected)

        wrong = connect(attach_path)
        send_json(
            wrong,
            ATTACH_OPEN,
            attach_open(session_id, "reclaim-owner", token="0" * 64),
        )
        frame_type, payload = receive_frame(wrong)
        assert frame_type == ERROR, (frame_type, payload)
        assert json.loads(payload)["code"] == "invalid_reclaim_token"
        wrong.close()

        resumed = connect(attach_path)
        send_json(
            resumed,
            ATTACH_OPEN,
            attach_open(session_id, "reclaim-owner", token=token),
        )
        frame_type, payload = receive_frame(resumed)
        assert frame_type == ATTACH_ACCEPTED, (frame_type, payload)
        resumed_accepted = json.loads(payload)
        assert resumed_accepted["owner"] == "reclaim-owner"
        assert resumed_accepted["reclaim_token"] != token
        frame_type, _ = receive_frame(resumed)
        assert frame_type == SNAPSHOT
        send_json(
            resumed,
            DETACH,
            {"protocol_version": PROTOCOL_VERSION, "hold_ttl_ms": 0},
        )
        while resumed.recv(4096):
            pass
        resumed.close()

        close_lease = send_control(
            args.socket,
            "LEASE_CLAIM",
            {
                "session_id": session_id,
                "owner": "reclaim-close-owner",
                "ttl_ms": 60000,
            },
            "reclaim-close-lease",
        )
        closed = send_control(
            args.socket,
            "SESSION_CLOSE",
            {
                "session_id": session_id,
                "owner": "reclaim-close-owner",
                "generation": close_lease["generation"],
            },
            "reclaim-close",
        )
        assert closed["state"] == "closed"
        worker_pid = None
        print(
            json.dumps(
                {
                    "case": "AUTH-033/AUTH-034",
                    "restart_grace": "pass",
                    "token_rotation": "pass",
                },
                separators=(",", ":"),
            )
        )
    finally:
        if active is not None:
            active.close()
        stop_qwed(second_qwed)
        stop_qwed(first_qwed)
        if worker_pid is not None:
            try:
                os.kill(worker_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


if __name__ == "__main__":
    main()
