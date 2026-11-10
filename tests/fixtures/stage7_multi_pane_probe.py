#!/usr/bin/env python3

import argparse
import json
import os
import signal
import tempfile
import time
from pathlib import Path

from stage7_resource_probe import (
    claim,
    crash_qwed,
    cgroup_path,
    process_gone,
    request,
    start_qwed,
    stop_qwed,
    wait_for,
    wait_snapshot,
)


OWNER = "stage7-multi"


def create_pane(socket_path, session_id, generation, marker):
    script = f"import time; print({marker!r}, flush=True); time.sleep(120)"
    created = request(
        socket_path,
        "PANE_CREATE",
        {
            "session_id": session_id,
            "program": "python3",
            "args": ["-c", script],
            "resource_profile": "tight",
            "owner": OWNER,
            "generation": generation,
        },
    )
    assert created.get("running") is True, created
    pane_snapshot = wait_for(
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
    assert pane_snapshot["pane_id"] == created["pane_id"], pane_snapshot
    return created


def control_status(socket_path):
    return request(socket_path, "CONTROL_STATUS", {})


def pane_task(status, pane_id):
    return next(
        (task for task in status["tasks"] if task.get("pane_id") == pane_id),
        None,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    qwed = None
    pane_ids = []
    worker_pids = []
    cgroups = []
    log_path = Path(f"/tmp/qwe-stage7-multi-{os.getpid()}.log")
    try:
        with tempfile.TemporaryDirectory(prefix="qwe-stage7-multi-") as log_dir:
            qwed = start_qwed(args.qwed, args.socket, log_path)
            created = request(
                args.socket,
                "SESSION_CREATE",
                {
                    "session_id": "multi-pane",
                    "program": "sh",
                    "args": ["-c", "echo PANE_ZERO; sleep 120"],
                    "resource_profile": "tight",
                },
            )
            assert created.get("running") is True, created
            wait_snapshot(args.socket, "multi-pane", "PANE_ZERO")
            generation = claim(args.socket, "multi-pane", OWNER)
            pane_ids.append(created["pane_id"])
            worker_pids.append(created["worker_pid"])
            cgroups.append(cgroup_path(created["pid"]))

            for marker in ("PANE_ONE", "PANE_TWO"):
                pane = create_pane(args.socket, "multi-pane", generation, marker)
                assert pane["pane_id"] not in pane_ids, pane
                assert pane["worker_pid"] not in worker_pids, pane
                pane_ids.append(pane["pane_id"])
                worker_pids.append(pane["worker_pid"])
                cgroups.append(cgroup_path(pane["pid"]))

            assert len(set(cgroups)) == 3, cgroups
            inventory = control_status(args.socket)
            assert len(
                [
                    pane
                    for pane in inventory["panes"]
                    if pane["session_id"] == "multi-pane"
                ]
            ) == 3, inventory
            assert len(
                [
                    worker
                    for worker in inventory["workers"]
                    if worker["session_id"] == "multi-pane"
                ]
            ) == 3, inventory

            crash_qwed(qwed)
            qwed = start_qwed(args.qwed, args.socket, log_path)
            for pane_id in pane_ids:
                recovered = request(
                    args.socket,
                    "PANE_STATUS",
                    {"pane_id": pane_id},
                )
                assert recovered.get("running") is True, recovered
            recovered_status = control_status(args.socket)
            assert len(
                [
                    worker
                    for worker in recovered_status["workers"]
                    if worker["session_id"] == "multi-pane"
                    and worker["health"] == "healthy"
                ]
            ) == 3, recovered_status

            os.kill(worker_pids[0], signal.SIGKILL)

            def crashed_pane_is_lost():
                status = control_status(args.socket)
                task = pane_task(status, pane_ids[0])
                return status if task is not None and task.get("state") == "lost" else None

            wait_for(
                crashed_pane_is_lost,
                "crashed Pane was not marked lost",
            )
            survivor = request(
                args.socket,
                "PANE_STATUS",
                {"pane_id": pane_ids[2]},
            )
            assert survivor["running"] is True, survivor
            session = next(
                session
                for session in control_status(args.socket)["sessions"]
                if session["id"] == "multi-pane"
            )
            assert session["state"] == "running", session

            for pane_id in pane_ids[1:]:
                killed = request(
                    args.socket,
                    "PANE_KILL",
                    {
                        "pane_id": pane_id,
                        "owner": OWNER,
                        "generation": generation,
                    },
                )
                assert killed.get("running") is False, killed
            request(
                args.socket,
                "SESSION_KILL",
                {
                    "session_id": "multi-pane",
                    "owner": OWNER,
                    "generation": generation,
                },
            )
            wait_for(
                lambda: all(not cgroup.exists() for cgroup in cgroups[1:]),
                "a Pane scope survived multi-Pane teardown",
            )
            print(
                json.dumps(
                    {
                        "case": "SHARED-002/SHARED-004/SHARED-006",
                        "multi_pane_inventory": "pass",
                        "qwed_restart_adoption": "pass",
                        "pane_failure_isolation": "pass",
                        "pane_addressed_status": "pass",
                        "complete_scope_cleanup": "pass",
                    },
                    separators=(",", ":"),
                )
            )
    except Exception:
        if "log_path" in locals():
            print(Path(log_path).read_text(errors="replace"), flush=True)
        raise
    finally:
        stop_qwed(qwed, args.socket)


if __name__ == "__main__":
    main()
