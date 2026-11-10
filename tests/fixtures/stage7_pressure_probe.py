#!/usr/bin/env python3

import argparse
import json
import math
import os
import statistics
import tempfile
import time
from pathlib import Path

from stage7_resource_probe import (
    claim,
    request,
    start_qwed,
    stop_qwed,
    wait_for,
    wait_snapshot,
)


OWNER = "stage7-pressure"


def create_pane(socket_path, session_id, generation, marker, program, arguments, policy):
    created = request(
        socket_path,
        "PANE_CREATE",
        {
            "session_id": session_id,
            "program": program,
            "args": arguments,
            "resource_policy": policy,
            "owner": OWNER,
            "generation": generation,
        },
    )
    assert created.get("running") is True, created
    if marker is not None:
        wait_for(
            lambda: (
                snapshot
                if marker
                in (
                    snapshot := request(
                        socket_path,
                        "PANE_ATTACH",
                        {"pane_id": created["pane_id"]},
                    )
                ).get("screen", "")
                else None
            ),
            f"Pane snapshot did not contain {marker}",
        )
    return created


def pane_status(socket_path, pane_id):
    return request(socket_path, "PANE_STATUS", {"pane_id": pane_id})


def wait_pane_exit(socket_path, pane_id, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = pane_status(socket_path, pane_id)
        if status.get("running") is False:
            return status
        time.sleep(0.02)
    raise RuntimeError(f"Pane {pane_id} did not exit under pressure: {status}")


def percentile(values, fraction):
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * fraction) - 1)
    return ordered[index]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    qwed = None
    pressure_panes = []
    try:
        with tempfile.TemporaryDirectory(prefix="qwe-stage7-pressure-") as log_dir:
            qwed = start_qwed(args.qwed, args.socket, Path(log_dir) / "qwed.log")
            control = request(
                args.socket,
                "SESSION_CREATE",
                {
                    "session_id": "pressure",
                    "program": "sh",
                    "args": ["-c", "echo CONTROL_READY; sleep 120"],
                    "resource_profile": "interactive",
                },
            )
            wait_snapshot(args.socket, "pressure", "CONTROL_READY")
            generation = claim(args.socket, "pressure", OWNER)

            memory = create_pane(
                args.socket,
                "pressure",
                generation,
                None,
                "python3",
                [
                    "-c",
                    (
                        "import time\n"
                        "b=bytearray(256*1024*1024)\n"
                        "for i in range(0,len(b),4096): b[i]=1\n"
                        "time.sleep(30)"
                    ),
                ],
                {
                    "memory_max_bytes": 64 * 1024 * 1024,
                    "memory_swap_max_bytes": 0,
                },
            )
            cpu = create_pane(
                args.socket,
                "pressure",
                generation,
                None,
                "sh",
                ["-c", "while :; do :; done"],
                {"cpu_seconds": 1},
            )
            tasks = create_pane(
                args.socket,
                "pressure",
                generation,
                None,
                "python3",
                [
                    "-c",
                    (
                        "import os,time\n"
                        "children=[]\n"
                        "denied=False\n"
                        "for _ in range(64):\n"
                        " try:\n"
                        "  pid=os.fork()\n"
                        "  if pid==0: time.sleep(10); os._exit(0)\n"
                        "  children.append(pid)\n"
                        " except OSError:\n"
                        "  denied=True; break\n"
                        " if denied: print('TASK_DENIED',flush=True)\n"
                        "time.sleep(10)\n"
                    ),
                ],
                {"tasks_max": 4},
            )
            pressure_panes = [
                memory["pane_id"],
                cpu["pane_id"],
                tasks["pane_id"],
            ]

            latencies = []
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline:
                started = time.perf_counter()
                pong = request(args.socket, "PING", {})
                latencies.append((time.perf_counter() - started) * 1000)
                assert pong["daemon"] == "ok", pong
                time.sleep(0.01)
            assert len(latencies) >= 20, len(latencies)

            memory_status = wait_pane_exit(args.socket, memory["pane_id"])
            cpu_status = wait_pane_exit(args.socket, cpu["pane_id"])
            tasks_status = wait_for(
                lambda: (
                    status
                    if (
                        status := pane_status(args.socket, tasks["pane_id"])
                    )["resource_metrics"]["last_violation"]
                    == "tasks_max"
                    else None
                ),
                "task-limit Pane did not report a pids.max violation",
                timeout=12,
            )
            assert (
                tasks_status["resource_metrics"]["last_violation"] == "tasks_max"
            ), tasks_status
            assert (
                memory_status["resource_metrics"]["last_violation"] == "memory_max"
            ), memory_status
            assert (
                cpu_status["resource_metrics"]["last_violation"] == "cpu_seconds"
            ), cpu_status
            control_status = pane_status(args.socket, control["pane_id"])
            assert control_status["running"] is True, control_status

            p95 = percentile(latencies, 0.95)
            threshold_ms = 1000.0
            assert p95 <= threshold_ms, (p95, latencies)
            for pane_id in pressure_panes:
                request(
                    args.socket,
                    "PANE_KILL",
                    {
                        "pane_id": pane_id,
                        "owner": OWNER,
                        "generation": generation,
                    },
                )
            request(
                args.socket,
                "SESSION_KILL",
                {
                    "session_id": "pressure",
                    "owner": OWNER,
                    "generation": generation,
                },
            )
            print(
                json.dumps(
                    {
                        "case": "RG-004",
                        "samples": len(latencies),
                        "p50_ms": round(statistics.median(latencies), 3),
                        "p95_ms": round(p95, 3),
                        "max_ms": round(max(latencies), 3),
                        "threshold_p95_ms": threshold_ms,
                        "memory_enforcement": "pass",
                        "cpu_enforcement": "pass",
                        "task_enforcement": "pass",
                        "control_pane_health": "pass",
                    },
                    separators=(",", ":"),
                )
            )
    finally:
        stop_qwed(qwed, args.socket)


if __name__ == "__main__":
    main()
