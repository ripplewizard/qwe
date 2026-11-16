#!/usr/bin/env python3

import argparse
import json
import os
import signal
import sqlite3
import tempfile
import time
import uuid
from pathlib import Path

from stage7_resource_probe import crash_qwed, request, send_control, stop_qwed
from stage7_soak_probe import start_qwed_at


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--history-panes", type=int, default=2000)
    parser.add_argument("--active-panes", type=int, default=1)
    parser.add_argument(
        "--history-state",
        choices=("lost", "closed"),
        default="lost",
    )
    parser.add_argument("--max-startup-ms", type=float, default=5000)
    parser.add_argument("--exercise-teardown", action="store_true")
    parser.add_argument("--max-teardown-ms", type=float, default=5000)
    args = parser.parse_args()
    if args.history_panes < 1:
        parser.error("--history-panes must be positive")
    if args.active_panes < 1:
        parser.error("--active-panes must be positive")

    qwed = None
    worker_pids = []
    with tempfile.TemporaryDirectory(prefix="qwe-reconcile-scale-") as run_root:
        root = Path(run_root)
        socket_path = root / "qwe.sock"
        store_path = root / "qwe.sqlite"
        log_path = root / "qwed.log"
        try:
            qwed = start_qwed_at(args.qwed, socket_path, store_path, log_path)
            created = request(
                str(socket_path),
                "SESSION_CREATE",
                {
                    "session_id": "reconcile-scale",
                    "program": "sh",
                    "args": ["-c", "while :; do sleep 60; done"],
                },
            )
            assert created.get("running") is True, created
            worker_pids.append(created["worker_pid"])
            if args.active_panes > 1:
                lease = request(
                    str(socket_path),
                    "LEASE_CLAIM",
                    {
                        "session_id": "reconcile-scale",
                        "owner": "stage7-history",
                        "ttl_ms": 60000,
                    },
                )
                for _ in range(1, args.active_panes):
                    pane = request(
                        str(socket_path),
                        "PANE_CREATE",
                        {
                            "session_id": "reconcile-scale",
                            "program": "sh",
                            "args": ["-c", "while :; do sleep 60; done"],
                            "owner": "stage7-history",
                            "generation": lease["generation"],
                        },
                    )
                    assert pane.get("running") is True, pane
                    worker_pids.append(pane["worker_pid"])

            crash_qwed(qwed)
            qwed = None
            now_ms = int(time.time() * 1000)
            with sqlite3.connect(store_path) as store:
                store.execute("PRAGMA foreign_keys=ON")
                template_scope = store.execute(
                    """
                    SELECT parent_scope_id, backend, requested_policy_json
                    FROM resource_scopes
                    WHERE target_kind = 'pane' AND target_id = ?
                    """,
                    (created["pane_id"],),
                ).fetchone()
                assert template_scope is not None, "active Pane has no resource scope"
                panes = []
                scopes = []
                workers = []
                tasks = []
                for index in range(args.history_panes):
                    pane_id = str(uuid.uuid4())
                    panes.append(
                        (
                            pane_id,
                            "reconcile-scale",
                            now_ms,
                            "normal",
                            args.history_panes + index,
                            1,
                            0,
                            None,
                            None,
                            None,
                            None,
                        )
                    )
                    scopes.append(
                        (
                            str(uuid.uuid4()),
                            "reconcile-scale",
                            "pane",
                            pane_id,
                            template_scope[0],
                            template_scope[1],
                            template_scope[2],
                            "closed",
                            "historical test pane",
                            now_ms,
                        )
                    )
                    workers.append(
                        (
                            pane_id,
                            2_000_000 + index,
                            2_000_000 + index,
                            "reconcile-scale:dead",
                            args.history_state,
                            now_ms,
                        )
                    )
                    tasks.append(
                        (
                            str(uuid.uuid4()),
                            pane_id,
                            2_000_000 + index,
                            2_000_000 + index,
                            "reconcile-scale:dead",
                            "exited" if args.history_state == "lost" else "stopped",
                            0,
                            None,
                            now_ms,
                            now_ms,
                        )
                    )
                store.executemany(
                    """
                    INSERT INTO panes (
                        id, session_id, updated_at_ms, kind, layout_order,
                        layout_weight, z_index, floating_x, floating_y,
                        floating_rows, floating_columns
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    panes,
                )
                store.executemany(
                    """
                    INSERT INTO resource_scopes (
                        id, session_id, target_kind, target_id, parent_scope_id,
                        backend, requested_policy_json, lifecycle_state,
                        cleanup_outcome, updated_at_ms
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    scopes,
                )
                store.executemany(
                    """
                    INSERT INTO pane_workers (
                        pane_id, pid, pgid, start_token, state, updated_at_ms
                    ) VALUES (?, ?, ?, ?, ?, ?)
                    """,
                    workers,
                )
                store.executemany(
                    """
                    INSERT INTO tasks (
                        id, pane_id, pid, pgid, start_token, state, exit_code,
                        exit_signal, started_at_ms, ended_at_ms
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    tasks,
                )

            started = time.perf_counter()
            qwed = start_qwed_at(args.qwed, socket_path, store_path, log_path)
            startup_ms = (time.perf_counter() - started) * 1000
            assert request(str(socket_path), "PING", {})["daemon"] == "ok"
            inventory = request(str(socket_path), "CONTROL_STATUS", {})
            session = next(
                item
                for item in inventory["sessions"]
                if item["id"] == "reconcile-scale"
            )
            assert session["state"] == "running", session

            result = {
                "case": "RG-003/LIVE-014",
                "active_panes": args.active_panes,
                "history_panes": args.history_panes,
                "startup_ms": round(startup_ms, 3),
                "max_startup_ms": args.max_startup_ms,
                "session_recovered": "pass",
            }
            if args.exercise_teardown:
                lease = request(
                    str(socket_path),
                    "LEASE_CLAIM",
                    {
                        "session_id": "reconcile-scale",
                        "owner": "stage7-history",
                        "ttl_ms": 60000,
                    },
                )
                started = time.perf_counter()
                killed = send_control(
                    str(socket_path),
                    "SESSION_KILL",
                    {
                        "session_id": "reconcile-scale",
                        "owner": "stage7-history",
                        "generation": lease["generation"],
                    },
                    timeout=args.max_teardown_ms / 1000 + 5,
                )
                teardown_ms = (time.perf_counter() - started) * 1000
                assert killed.get("state") == "closed", killed
                expected_panes = args.active_panes + args.history_panes
                returned_panes = len(killed.get("panes", []))
                assert returned_panes == expected_panes, {
                    "expected_panes": expected_panes,
                    "returned_panes": returned_panes,
                    "result_keys": sorted(killed),
                }
                result.update(
                    {
                        "teardown_ms": round(teardown_ms, 3),
                        "max_teardown_ms": args.max_teardown_ms,
                        "session_closed": "pass",
                    }
                )
            print(json.dumps(result, sort_keys=True, separators=(",", ":")))
            if startup_ms > args.max_startup_ms or (
                args.exercise_teardown
                and teardown_ms > args.max_teardown_ms
            ):
                raise SystemExit(1)
        finally:
            if qwed is not None:
                stop_qwed(qwed, str(socket_path))
            for worker_pid in worker_pids:
                try:
                    os.kill(worker_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    main()
