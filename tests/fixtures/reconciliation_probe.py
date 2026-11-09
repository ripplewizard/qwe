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
import uuid
from pathlib import Path


PROTOCOL_VERSION = 1


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


def send_control(path, operation, payload, key=None):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"reconciliation-{time.time_ns()}",
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


def wait_for_socket(path, process):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"process exited with status {process.returncode}")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(str(path))
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for {path}")


def start_qwed(binary, socket_path, store_path):
    process = subprocess.Popen(
        [
            binary,
            "--socket",
            socket_path,
            "--store",
            store_path,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    wait_for_socket(socket_path, process)
    return process


def stop_qwed(process):
    if process is None or process.poll() is not None:
        return
    os.kill(process.pid, signal.SIGKILL)
    process.wait(timeout=5)


def runtime_socket(runtime_dir, session_id):
    encoded = session_id.encode().hex()
    return runtime_dir / f"{encoded}.sock"


def start_worker(binary, runtime_dir, session_id, socket_path=None):
    socket_path = Path(socket_path) if socket_path else runtime_socket(
        runtime_dir, session_id
    )
    process = subprocess.Popen(
        [
            binary,
            "--worker-socket",
            str(socket_path),
            "--worker-session-id",
            session_id,
            "--worker-program",
            "sh",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    wait_for_socket(socket_path, process)
    return process, socket_path


def create_session(
    path,
    session_id,
    program="sh",
    arguments=None,
    require_running=True,
):
    created = send_control(
        path,
        "SESSION_CREATE",
        {
            "session_id": session_id,
            "program": program,
            "args": arguments or [],
        },
        key=f"create-{session_id}",
    )
    if require_running:
        assert created["running"] is True, created
    return created


def close_session(path, session_id):
    lease = send_control(
        path,
        "LEASE_CLAIM",
        {
            "session_id": session_id,
            "owner": f"{session_id}-close-owner",
            "ttl_ms": 60000,
        },
        key=f"close-lease-{session_id}",
    )
    closed = send_control(
        path,
        "SESSION_CLOSE",
        {
            "session_id": session_id,
            "owner": f"{session_id}-close-owner",
            "generation": lease["generation"],
        },
        key=f"close-{session_id}",
    )
    assert closed["state"] == "closed", closed


def status(path):
    result = send_control(path, "CONTROL_STATUS", {})
    assert result["daemon"] == "ok", result
    return result


def assert_incident(report, disposition, session_id, health=None):
    entries = report["reconciliation"][f"{disposition}_runtime_entries"]
    matching = [
        entry
        for entry in entries
        if entry["session_id"] == session_id
        and (health is None or entry["health"] == health)
    ]
    assert matching, (disposition, session_id, health, entries)
    return matching


def kill_process(process):
    if process is not None and process.poll() is None:
        try:
            os.kill(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--worker", required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--store", required=True)
    args = parser.parse_args()
    runtime_dir = Path(f"{args.socket}.runtime")
    runtime_dir.mkdir(mode=0o700, exist_ok=True)
    qwed = None
    workers = []
    try:
        orphan_id = "orphan-healthy"
        orphan, orphan_socket = start_worker(args.worker, runtime_dir, orphan_id)
        workers.append(orphan)
        qwed = start_qwed(args.qwed, args.socket, args.store)
        report = status(args.socket)
        assert_incident(report, "orphaned", orphan_id, "healthy")
        assert orphan_socket.exists()
        rejected = send_control(
            args.socket,
            "SESSION_STATUS",
            {"session_id": orphan_id},
        )
        assert rejected["code"] == "terminal_lost", rejected
        stop_qwed(qwed)
        qwed = None

        os.kill(orphan.pid, signal.SIGSTOP)
        qwed = start_qwed(args.qwed, args.socket, args.store)
        report = status(args.socket)
        assert_incident(report, "orphaned", orphan_id, "unreachable")
        assert orphan_socket.exists()
        os.kill(orphan.pid, signal.SIGCONT)
        kill_process(orphan)
        workers.remove(orphan)
        stop_qwed(qwed)
        qwed = None
        orphan_socket.unlink(missing_ok=True)
        orphan_socket.with_suffix(".a").unlink(missing_ok=True)

        closed_id = "closed-conflict"
        qwed = start_qwed(args.qwed, args.socket, args.store)
        closed_created = create_session(args.socket, closed_id)
        close_session(args.socket, closed_id)
        assert closed_created["worker_pid"]
        stop_qwed(qwed)
        qwed = None
        conflict_worker, conflict_socket = start_worker(
            args.worker, runtime_dir, closed_id
        )
        workers.append(conflict_worker)
        qwed = start_qwed(args.qwed, args.socket, args.store)
        report = status(args.socket)
        assert_incident(report, "conflict", closed_id, "healthy")
        closed_session = [
            session
            for session in report["sessions"]
            if session["id"] == closed_id
        ]
        assert len(closed_session) == 1 and closed_session[0]["state"] == "closed"
        assert not conflict_socket.exists()
        quarantined = list(runtime_dir.glob(f"{closed_id.encode().hex()}.sock.conflict-*"))
        assert quarantined, list(runtime_dir.iterdir())
        kill_process(conflict_worker)
        workers.remove(conflict_worker)
        stop_qwed(qwed)
        qwed = None

        mismatch_id = "identity-mismatch"
        qwed = start_qwed(args.qwed, args.socket, args.store)
        mismatch_created = create_session(args.socket, mismatch_id)
        mismatch_worker_pid = mismatch_created["worker_pid"]
        stop_qwed(qwed)
        qwed = None
        with sqlite3.connect(args.store) as connection:
            connection.execute(
                "UPDATE sessions SET worker_start_token = ? WHERE id = ?",
                ("stale-start-token", mismatch_id),
            )
            connection.commit()
        qwed = start_qwed(args.qwed, args.socket, args.store)
        report = status(args.socket)
        assert_incident(report, "conflict", mismatch_id, "healthy")
        mismatch_session = [
            session
            for session in report["sessions"]
            if session["id"] == mismatch_id
        ]
        assert len(mismatch_session) == 1 and mismatch_session[0]["state"] == "lost"
        mismatch_task = [
            task
            for task in report["tasks"]
            if task["session_id"] == mismatch_id
        ]
        assert len(mismatch_task) == 1 and mismatch_task[0]["state"] == "lost"
        try:
            os.kill(mismatch_worker_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stop_qwed(qwed)
        qwed = None

        duplicate_id = "duplicate-worker"
        qwed = start_qwed(args.qwed, args.socket, args.store)
        create_session(args.socket, duplicate_id)
        stop_qwed(qwed)
        qwed = None
        first_socket = runtime_socket(runtime_dir, duplicate_id)
        duplicate_socket = runtime_dir / "duplicate-worker.sock"
        duplicate, duplicate_socket = start_worker(
            args.worker,
            runtime_dir,
            duplicate_id,
            duplicate_socket,
        )
        workers.append(duplicate)
        qwed = start_qwed(args.qwed, args.socket, args.store)
        report = status(args.socket)
        duplicate_entries = assert_incident(
            report, "conflict", duplicate_id, "healthy"
        )
        assert len(duplicate_entries) == 2, duplicate_entries
        assert not first_socket.exists()
        assert not duplicate_socket.exists()
        rejected = send_control(
            args.socket,
            "SESSION_STATUS",
            {"session_id": duplicate_id},
        )
        assert rejected["code"] == "terminal_lost", rejected
        assert (
            list(runtime_dir.glob(f"{duplicate_id.encode().hex()}.sock.conflict-*"))
            and list(runtime_dir.glob("duplicate-worker.sock.conflict-*"))
        )

        exited_id = "exited-unreachable"
        exited_created = create_session(
            args.socket,
            exited_id,
            arguments=["-c", "sleep 0.1; exit 0"],
            require_running=False,
        )
        exited_worker_pid = exited_created.get("worker_pid")
        assert exited_worker_pid is not None, exited_created
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            exited_report = status(args.socket)
            exited_sessions = [
                session
                for session in exited_report["sessions"]
                if session["id"] == exited_id
            ]
            exited_tasks = [
                task
                for task in exited_report["tasks"]
                if task["session_id"] == exited_id
            ]
            if (
                exited_sessions
                and exited_sessions[0]["state"] == "exited"
                and exited_tasks
                and exited_tasks[0]["state"] == "exited"
            ):
                break
            time.sleep(0.02)
        else:
            raise AssertionError("exited Session did not settle")
        stop_qwed(qwed)
        qwed = None
        os.kill(exited_worker_pid, signal.SIGSTOP)
        qwed = start_qwed(args.qwed, args.socket, args.store)
        report = status(args.socket)
        exited_sessions = [
            session
            for session in report["sessions"]
            if session["id"] == exited_id
        ]
        exited_tasks = [
            task
            for task in report["tasks"]
            if task["session_id"] == exited_id
        ]
        assert exited_sessions[0]["state"] == "exited", exited_sessions
        assert exited_tasks[0]["state"] == "exited", exited_tasks
        unreachable = [
            worker
            for worker in report["workers"]
            if worker["session_id"] == exited_id
        ]
        assert unreachable[0]["health"] == "unreachable", unreachable
        os.kill(exited_worker_pid, signal.SIGCONT)
        try:
            os.kill(exited_worker_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

        stop_qwed(qwed)
        qwed = None
        historical_id = "historical-closed"
        historical_task_id = str(uuid.uuid4())
        historical_pane_id = str(uuid.uuid4())
        old_ms = int(time.time() * 1000) - 31 * 24 * 60 * 60 * 1000
        with sqlite3.connect(args.store) as connection:
            connection.execute(
                "INSERT INTO sessions "
                "(id, owner, state, updated_at_ms) VALUES (?, 'local', 'closed', ?)",
                (historical_id, old_ms),
            )
            connection.execute(
                "INSERT INTO tasks "
                "(id, session_id, pid, pgid, start_token, state, "
                "started_at_ms, ended_at_ms) "
                "VALUES (?, ?, 1, 1, 'historical', 'stopped', ?, ?)",
                (historical_task_id, historical_id, old_ms, old_ms),
            )
            connection.execute(
                "INSERT INTO panes (id, session_id, updated_at_ms) VALUES (?, ?, ?)",
                (historical_pane_id, historical_id, old_ms),
            )
            connection.executemany(
                "INSERT INTO sessions "
                "(id, owner, state, updated_at_ms) VALUES (?, 'local', 'closed', ?)",
                (
                    (f"history-filler-{index}", old_ms + index + 1)
                    for index in range(10_001)
                ),
            )
            connection.commit()
        qwed = start_qwed(args.qwed, args.socket, args.store)
        with sqlite3.connect(args.store) as connection:
            historical_session = connection.execute(
                "SELECT COUNT(*) FROM sessions WHERE id = ?",
                (historical_id,),
            ).fetchone()[0]
            historical_task = connection.execute(
                "SELECT COUNT(*) FROM tasks WHERE id = ?",
                (historical_task_id,),
            ).fetchone()[0]
            historical_pane = connection.execute(
                "SELECT COUNT(*) FROM panes WHERE id = ?",
                (historical_pane_id,),
            ).fetchone()[0]
        assert historical_session == 0
        assert historical_task == 0
        assert historical_pane == 0

        print(
            json.dumps(
                {
                    "case": "LIVE-004/LIVE-005/LIVE-006/LIVE-007/LIVE-008/LIVE-010/LIVE-016/HIST-002",
                    "healthy_orphan": "pass",
                    "unreachable_orphan": "pass",
                    "closed_conflict": "pass",
                    "identity_mismatch": "pass",
                    "duplicate_workers": "pass",
                    "exited_unreachable": "pass",
                    "runtime_entries_preserved": "pass",
                    "history_retention": "pass",
                },
                separators=(",", ":"),
            )
        )
    finally:
        stop_qwed(qwed)
        for worker in workers:
            kill_process(worker)


if __name__ == "__main__":
    main()
