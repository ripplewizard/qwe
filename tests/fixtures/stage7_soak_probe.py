#!/usr/bin/env python3

import argparse
import contextlib
import json
import math
import os
import signal
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from release_benchmark import rss_bytes
import attach_channel_probe as protocol
from stage7_resource_probe import (
    claim,
    crash_qwed,
    cgroup_path,
    request,
    stop_qwed,
    wait_for,
)


OWNER = "stage7-soak"


def percentile(values, fraction):
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * fraction) - 1)
    return ordered[index]


def pane_status(socket_path, pane_id):
    return request(socket_path, "PANE_STATUS", {"pane_id": pane_id})


def attach_latency(attach_path, session_id, owner, generation):
    started = time.perf_counter()
    stream = protocol.connect(attach_path)
    try:
        protocol.send_json(
            stream,
            protocol.ATTACH_OPEN,
            protocol.open_frame(
                session_id,
                owner,
                lease_generation=generation,
            ),
        )
        frame_type, payload = protocol.receive_frame(stream)
        assert frame_type == protocol.ATTACH_ACCEPTED, payload
        frame_type, payload = protocol.receive_frame(stream)
        assert frame_type == protocol.SNAPSHOT, payload
        snapshot = json.loads(payload)
        protocol.send_json(
            stream,
            protocol.RESIZE,
            {
                "protocol_version": protocol.PROTOCOL_VERSION,
                "rows": 25,
                "columns": 81,
            },
        )
        deadline = time.monotonic() + 2
        while True:
            frame_type, payload = protocol.receive_frame(stream)
            if frame_type == protocol.RESIZED:
                resized = json.loads(payload)
                assert resized["rows"] == 25, resized
                assert resized["columns"] == 81, resized
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("attach resize was not acknowledged")
        protocol.send_payload(stream, protocol.INPUT, b"soak-input\n")
        elapsed_ms = (time.perf_counter() - started) * 1000
        protocol.send_json(
            stream,
            protocol.DETACH,
            {"protocol_version": protocol.PROTOCOL_VERSION, "hold_ttl_ms": 0},
        )
        protocol.expect_eof(stream)
        return elapsed_ms, snapshot["next_stream_offset"]
    finally:
        stream.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--sessions", type=int, default=8)
    parser.add_argument("--panes-per-session", type=int, default=2)
    parser.add_argument("--duration-seconds", type=float, default=30)
    parser.add_argument("--restart-interval-seconds", type=float, default=10)
    parser.add_argument("--max-p95-ms", type=float, default=1000)
    parser.add_argument("--max-p99-ms", type=float, default=2000)
    parser.add_argument("--max-rss-bytes", type=int, default=4 * 1024**3)
    parser.add_argument("--run-root", type=Path)
    parser.add_argument("--progress-interval", type=float, default=60)
    parser.add_argument("--started-at")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.sessions < 1:
        parser.error("--sessions must be positive")
    if args.panes_per_session < 1:
        parser.error("--panes-per-session must be positive")
    if args.duration_seconds < 0:
        parser.error("--duration-seconds must be non-negative")

    qwed = None
    sessions = {}
    scopes = []
    attach_samples = []
    ping_samples = []
    memory_samples = []
    restarts = 0
    churn_panes = 0
    churn_state = None
    resize_ops = 0
    input_ops = 0
    pane_snapshot_ops = 0
    control_ops = 0
    task_status_ops = 0
    task_history_ops = 0
    lease_status_ops = 0
    lease_touch_ops = 0
    resource_updates = 0
    soak_socket = Path("/tmp/qwe-stage7-soak.sock")
    try:
        with contextlib.ExitStack() as stack:
            runtime_root = Path(
                stack.enter_context(
                    tempfile.TemporaryDirectory(prefix="qwe-soak-runtime-")
                )
            )
            if args.run_root is not None:
                artifact_root = args.run_root.resolve()
                artifact_root.mkdir(parents=True, exist_ok=True)
            else:
                artifact_root = runtime_root
            socket_path = runtime_root / "qwe.sock"
            soak_socket = socket_path
            store_path = artifact_root / "qwe.sqlite"
            log_path = artifact_root / "qwed.log"
            attach_path = f"{socket_path}.attach"
            qwed = start_qwed_at(args.qwed, socket_path, store_path, log_path)
            output_program = (
                "import os,select,time\n"
                "value=0\n"
                "while True:\n"
                " value+=1\n"
                " os.write(1, ('SOAK_%08d\\n' % value).encode())\n"
                " if select.select([0], [], [], 0)[0]: os.read(0, 4096)\n"
                " time.sleep(0.005)\n"
            )
            for index in range(args.sessions):
                session_id = f"soak-{index:02d}"
                created = request(
                    str(socket_path),
                    "SESSION_CREATE",
                    {
                        "session_id": session_id,
                        "program": sys.executable,
                        "args": ["-c", output_program],
                        "resource_profile": "tight",
                    },
                )
                assert created.get("running") is True, created
                generation = claim(str(socket_path), session_id, OWNER)
                panes = [
                    {
                        "pane_id": created["pane_id"],
                        "task_id": created["task_id"],
                        "task_pid": created["pid"],
                        "worker_pid": created["worker_pid"],
                    }
                ]
                scopes.append(cgroup_path(created["pid"]))
                for pane_index in range(1, args.panes_per_session):
                    pane = request(
                        str(socket_path),
                        "PANE_CREATE",
                        {
                            "session_id": session_id,
                            "program": sys.executable,
                            "args": ["-c", output_program],
                            "resource_profile": "tight",
                            "owner": OWNER,
                            "generation": generation,
                        },
                    )
                    assert pane.get("running") is True, pane
                    panes.append(
                        {
                            "pane_id": pane["pane_id"],
                            "task_id": pane["task_id"],
                            "task_pid": pane["pid"],
                            "worker_pid": pane["worker_pid"],
                        }
                    )
                    scopes.append(cgroup_path(pane["pid"]))
                sessions[session_id] = {
                    "generation": generation,
                    "panes": panes,
                }

            churn_created = request(
                str(socket_path),
                "SESSION_CREATE",
                {
                    "session_id": "soak-churn",
                    "program": "sh",
                    "args": ["-c", "while :; do sleep 60; done"],
                    "resource_profile": "tight",
                },
            )
            assert churn_created.get("running") is True, churn_created
            churn_lease = request(
                str(socket_path),
                "LEASE_CLAIM",
                {
                    "session_id": "soak-churn",
                    "owner": OWNER,
                    "ttl_ms": 86400000,
                },
            )
            churn_state = {
                "generation": churn_lease["generation"],
                "pane_id": churn_created["pane_id"],
                "task_pid": churn_created["pid"],
                "worker_pid": churn_created["worker_pid"],
            }
            scopes.append(cgroup_path(churn_created["pid"]))

            memory_samples.append(
                rss_bytes(
                    [
                        qwed.pid,
                        *[
                            pane["worker_pid"]
                            for session in sessions.values()
                            for pane in session["panes"]
                        ],
                        *[
                            pane["task_pid"]
                            for session in sessions.values()
                            for pane in session["panes"]
                        ],
                    ]
                )
            )
            deadline = time.monotonic() + args.duration_seconds
            next_restart = (
                time.monotonic() + args.restart_interval_seconds
                if args.restart_interval_seconds > 0
                else float("inf")
            )
            next_progress = time.monotonic() + args.progress_interval
            cycle = 0
            started_at = args.started_at or time.strftime(
                "%Y-%m-%dT%H:%M:%S%z"
            )
            progress_path = artifact_root / "progress.json"

            def write_progress(progress_status):
                progress = {
                    "status": progress_status,
                    "started_at": started_at,
                    "updated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                    "elapsed_seconds": round(
                        args.duration_seconds - max(0, deadline - time.monotonic()),
                        3,
                    ),
                    "planned_seconds": args.duration_seconds,
                    "cycle": cycle,
                    "restarts": restarts,
                    "churn_panes": churn_panes,
                    "resize_ops": resize_ops,
                    "input_ops": input_ops,
                    "pane_snapshot_ops": pane_snapshot_ops,
                    "control_ops": control_ops,
                    "task_status_ops": task_status_ops,
                    "task_history_ops": task_history_ops,
                    "lease_status_ops": lease_status_ops,
                    "lease_touch_ops": lease_touch_ops,
                    "resource_updates": resource_updates,
                    "ping_samples": len(ping_samples),
                    "attach_samples": len(attach_samples),
                    "max_rss_bytes": max(memory_samples) if memory_samples else 0,
                }
                temporary = progress_path.with_suffix(".json.tmp")
                temporary.write_text(
                    json.dumps(progress, sort_keys=True, separators=(",", ":"))
                    + "\n"
                )
                temporary.replace(progress_path)

            write_progress("running")
            while time.monotonic() < deadline:
                for session_id, state in sessions.items():
                    started = time.perf_counter()
                    request(str(socket_path), "PING", {})
                    ping_samples.append((time.perf_counter() - started) * 1000)
                    elapsed_ms, _ = attach_latency(
                        attach_path,
                        session_id,
                        OWNER,
                        state["generation"],
                    )
                    attach_samples.append(elapsed_ms)
                    resize_ops += 1
                    input_ops += 1
                    for pane in state["panes"]:
                        started = time.perf_counter()
                        status = pane_status(str(socket_path), pane["pane_id"])
                        attach_samples.append((time.perf_counter() - started) * 1000)
                        assert status.get("running") is True, status
                        metrics = status["resource_metrics"]
                        assert len(metrics["events"]) <= 32, metrics
                    if len(state["panes"]) > 1:
                        started = time.perf_counter()
                        snapshot = request(
                            str(socket_path),
                            "PANE_ATTACH",
                            {"pane_id": state["panes"][1]["pane_id"]},
                        )
                        attach_samples.append((time.perf_counter() - started) * 1000)
                        assert snapshot.get("pane_id") == state["panes"][1]["pane_id"], snapshot
                        pane_snapshot_ops += 1

                if cycle % 10 == 0:
                    inventory = request(str(socket_path), "CONTROL_STATUS", {})
                    assert len(inventory["panes"]) >= args.sessions * args.panes_per_session, inventory
                    control_ops += 1
                    request(str(socket_path), "TASK_HISTORY", {})
                    task_history_ops += 1
                    for session_id, state in sessions.items():
                        request(
                            str(socket_path),
                            "LEASE_STATUS",
                            {"session_id": session_id},
                        )
                        lease_status_ops += 1
                        request(
                            str(socket_path),
                            "TASK_STATUS",
                            {"task_id": state["panes"][0]["task_id"]},
                        )
                        task_status_ops += 1
                    request(
                        str(socket_path),
                        "LEASE_TOUCH",
                        {
                            "session_id": "soak-churn",
                            "owner": OWNER,
                            "generation": churn_state["generation"],
                            "ttl_ms": 86400000,
                        },
                    )
                    lease_touch_ops += 1

                if cycle % 20 == 0:
                    policy = request(
                        str(socket_path),
                        "SESSION_RESOURCE_UPDATE",
                        {
                            "session_id": "soak-churn",
                            "owner": OWNER,
                            "generation": churn_state["generation"],
                            "resource_policy": {
                                "memory_max_bytes": (
                                    256 if (cycle // 20) % 2 == 0 else 240
                                )
                                * 1024
                                * 1024,
                                "tasks_max": 72 if (cycle // 20) % 2 == 0 else 64,
                            },
                        },
                    )
                    assert policy.get("resource_policy"), policy
                    resource_updates += 1

                if cycle % 5 == 0:
                    churn = request(
                        str(socket_path),
                        "PANE_CREATE",
                        {
                            "session_id": "soak-churn",
                            "program": "/bin/true",
                            "resource_profile": "tight",
                            "owner": OWNER,
                            "generation": churn_state["generation"],
                        },
                    )
                    assert "pane_id" in churn, churn
                    churn_panes += 1
                    request(
                        str(socket_path),
                        "PANE_KILL",
                        {
                            "pane_id": churn["pane_id"],
                            "owner": OWNER,
                            "generation": churn_state["generation"],
                        },
                    )

                memory_samples.append(
                    rss_bytes(
                        [
                            qwed.pid,
                            *[
                                pane["worker_pid"]
                                for session in sessions.values()
                                for pane in session["panes"]
                            ],
                            *[
                                pane["task_pid"]
                                for session in sessions.values()
                                for pane in session["panes"]
                            ],
                        ]
                    )
                )
                if time.monotonic() >= next_restart:
                    crash_qwed(qwed)
                    qwed = start_qwed_at(
                        args.qwed,
                        socket_path,
                        store_path,
                        log_path,
                    )
                    restarts += 1
                    next_restart = time.monotonic() + args.restart_interval_seconds
                    for session_id, state in sessions.items():
                        for pane in state["panes"]:
                            status = pane_status(str(socket_path), pane["pane_id"])
                            assert status.get("running") is True, status
                if time.monotonic() >= next_progress:
                    write_progress("running")
                    next_progress = time.monotonic() + args.progress_interval
                cycle += 1
                time.sleep(0.05)

            for session_id, state in sessions.items():
                lease = request(
                    str(socket_path),
                    "LEASE_CLAIM",
                    {
                        "session_id": session_id,
                        "owner": OWNER,
                        "ttl_ms": 60000,
                    },
                )
                state["generation"] = lease["generation"]
                killed = request(
                    str(socket_path),
                    "SESSION_KILL",
                    {
                        "session_id": session_id,
                        "owner": OWNER,
                        "generation": state["generation"],
                    },
                )
                assert killed.get("state") == "closed", killed
            lease = request(
                str(socket_path),
                "LEASE_CLAIM",
                {
                    "session_id": "soak-churn",
                    "owner": OWNER,
                    "ttl_ms": 60000,
                },
            )
            churn_state["generation"] = lease["generation"]
            killed = request(
                str(socket_path),
                "SESSION_KILL",
                {
                    "session_id": "soak-churn",
                    "owner": OWNER,
                    "generation": churn_state["generation"],
                },
            )
            assert killed.get("state") == "closed", killed
            wait_for(
                lambda: all(not scope.exists() for scope in scopes),
                "a Pane scope survived soak teardown",
                timeout=30,
            )
            runtime_dir = Path(f"{socket_path}.runtime")
            wait_for(
                lambda: not any(runtime_dir.glob("p*.sock")),
                "a worker socket survived soak teardown",
                timeout=15,
            )
            write_progress("completed")
    finally:
        stop_qwed(qwed, str(soak_socket))
        if qwed is not None and qwed.poll() is None:
            qwed.kill()
            qwed.wait(timeout=5)
        tracked_workers = [
            pane["worker_pid"]
            for session in sessions.values()
            for pane in session["panes"]
        ]
        if churn_state is not None:
            tracked_workers.append(churn_state["worker_pid"])
        for worker_pid in tracked_workers:
            try:
                os.kill(worker_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for scope in scopes:
            unit = scope.name
            subprocess.run(
                ["systemctl", "--user", "stop", unit],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            subprocess.run(
                ["systemctl", "--user", "reset-failed", unit],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )

    result = {
        "case": "RG-006/PERF-001",
        "build_profile": "release"
        if "/release/" in os.path.abspath(args.qwed)
        else "debug",
        "sessions": args.sessions,
        "panes_per_session": args.panes_per_session,
        "duration_seconds": args.duration_seconds,
        "restarts": restarts,
        "churn_panes": churn_panes,
        "resize_ops": resize_ops,
        "input_ops": input_ops,
        "pane_snapshot_ops": pane_snapshot_ops,
        "control_ops": control_ops,
        "task_status_ops": task_status_ops,
        "task_history_ops": task_history_ops,
        "lease_status_ops": lease_status_ops,
        "lease_touch_ops": lease_touch_ops,
        "resource_updates": resource_updates,
        "ping_samples": len(ping_samples),
        "attach_samples": len(attach_samples),
        "ping_p50_ms": round(statistics.median(ping_samples), 3)
        if ping_samples
        else 0,
        "ping_p95_ms": round(percentile(ping_samples, 0.95), 3)
        if ping_samples
        else 0,
        "attach_p50_ms": round(statistics.median(attach_samples), 3)
        if attach_samples
        else 0,
        "attach_p95_ms": round(percentile(attach_samples, 0.95), 3)
        if attach_samples
        else 0,
        "attach_p99_ms": round(percentile(attach_samples, 0.99), 3)
        if attach_samples
        else 0,
        "max_rss_bytes": max(memory_samples) if memory_samples else 0,
        "max_rss_limit_bytes": args.max_rss_bytes,
        "max_p95_ms": args.max_p95_ms,
        "max_p99_ms": args.max_p99_ms,
    }
    rendered = json.dumps(result, sort_keys=True, separators=(",", ":"))
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered + "\n")
    print(rendered)
    if (
        result["attach_p95_ms"] > args.max_p95_ms
        or result["attach_p99_ms"] > args.max_p99_ms
        or result["max_rss_bytes"] > args.max_rss_bytes
    ):
        raise SystemExit(1)


def start_qwed_at(binary, socket_path, store_path, log_path):
    log = open(log_path, "ab", buffering=0)
    process = subprocess.Popen(
        [
            binary,
            "--socket",
            str(socket_path),
            "--store",
            str(store_path),
        ],
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    process.qwe_log = str(log_path)
    wait_for(
        lambda: _socket_ready(socket_path),
        f"timed out waiting for qwed on {socket_path}",
        timeout=15,
    )
    return process


def _socket_ready(socket_path):
    import socket

    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.connect(str(socket_path))
        return True
    except OSError:
        return False


if __name__ == "__main__":
    main()
