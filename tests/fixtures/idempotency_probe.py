#!/usr/bin/env python3

import argparse
import json
import os
import signal
import socket
import sqlite3
import struct
import subprocess
import time


PROTOCOL_VERSION = 1
MAX_FRAME_BYTES = 8 * 1024 * 1024


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


def send_request(path, operation, payload, idempotency_key=None):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"idempotency-{time.time_ns()}",
        "idempotency_key": idempotency_key,
        "operation": operation,
        "payload": payload,
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        header = receive_exact(stream, 4)
        length = struct.unpack(">I", header)[0]
        assert length <= MAX_FRAME_BYTES
        return json.loads(receive_exact(stream, length))


def result_of(response):
    if "Ok" in response["result"]:
        return response["result"]["Ok"]
    if "Err" in response["result"]:
        return response["result"]["Err"]
    raise AssertionError(response)


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
    if process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGKILL if force else signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    session_id = "idempotency-probe"
    create_payload = {
        "session_id": session_id,
        "program": "sh",
        "args": [],
    }
    create_key = "create-session-once"
    first_qwed = None
    second_qwed = None
    worker_pid = None
    try:
        first_qwed = start_qwed(args.qwed, args.socket)
        first = send_request(
            args.socket,
            "SESSION_CREATE",
            create_payload,
            create_key,
        )
        first_result = result_of(first)
        assert "session_id" in first_result, first
        worker_pid = first_result["worker_pid"]

        replay = send_request(
            args.socket,
            "SESSION_CREATE",
            create_payload,
            create_key,
        )
        assert result_of(replay) == first_result, (first, replay)

        conflict = send_request(
            args.socket,
            "SESSION_CREATE",
            {"session_id": "different-session", "program": "sh", "args": []},
            create_key,
        )
        assert result_of(conflict)["code"] == "idempotency_conflict", conflict

        missing = send_request(
            args.socket,
            "SESSION_CREATE",
            {"session_id": "missing-key", "program": "sh", "args": []},
        )
        assert result_of(missing)["code"] == "invalid_request", missing

        stop_qwed(first_qwed, force=True)
        first_qwed = None
        with sqlite3.connect(f"{args.socket}.sqlite") as connection:
            connection.execute(
                "UPDATE idempotency_records SET state = 'pending', response_json = NULL "
                "WHERE idempotency_key = ?",
                (create_key,),
            )
            connection.commit()
        second_qwed = start_qwed(args.qwed, args.socket)
        recovered = send_request(
            args.socket,
            "SESSION_CREATE",
            create_payload,
            create_key,
        )
        assert result_of(recovered) == first_result, (first, recovered)
        after_restart = send_request(
            args.socket,
            "SESSION_CREATE",
            create_payload,
            create_key,
        )
        assert result_of(after_restart) == first_result, (first, after_restart)

        status = send_request(args.socket, "CONTROL_STATUS", {})
        status_result = result_of(status)
        sessions = [
            session
            for session in status_result["sessions"]
            if session["id"] == session_id
        ]
        assert len(sessions) == 1, status

        close_lease = send_request(
            args.socket,
            "LEASE_CLAIM",
            {
                "session_id": session_id,
                "owner": "idempotency-owner",
                "ttl_ms": 60000,
            },
            "close-authority",
        )
        close_generation = result_of(close_lease)["generation"]
        closed = send_request(
            args.socket,
            "SESSION_CLOSE",
            {
                "session_id": session_id,
                "owner": "idempotency-owner",
                "generation": close_generation,
            },
            "close-idempotency-probe",
        )
        assert result_of(closed)["state"] == "closed", closed
        worker_pid = None
        print(
            json.dumps(
                {
                    "case": "IDEMP-001/002/003",
                    "replay": "pass",
                    "pending_create_recovery": "pass",
                    "restart_replay": "pass",
                    "conflict": "pass",
                    "missing_key": "pass",
                },
                separators=(",", ":"),
            )
        )
    finally:
        if second_qwed is not None:
            stop_qwed(second_qwed)
        if first_qwed is not None:
            stop_qwed(first_qwed)
        if worker_pid is not None:
            try:
                os.kill(worker_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


if __name__ == "__main__":
    main()
