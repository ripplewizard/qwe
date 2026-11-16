#!/usr/bin/env python3

import argparse
import json
import sqlite3
import tempfile
import time
from pathlib import Path

from stage7_resource_probe import (
    claim,
    crash_qwed,
    request,
    start_qwed,
    stop_qwed,
)


OWNER = "qwed-restart-volume"


def idempotency_count(store_path):
    with sqlite3.connect(store_path) as connection:
        return connection.execute(
            "SELECT COUNT(*) FROM idempotency_records"
        ).fetchone()[0]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    socket_path = args.socket
    store_path = f"{socket_path}.sqlite"
    qwed = None
    try:
        with tempfile.TemporaryDirectory(prefix="qwed-restart-volume-") as log_dir:
            log_path = Path(log_dir) / "qwed.log"
            qwed = start_qwed(args.qwed, socket_path, log_path)
            created = request(
                socket_path,
                "SESSION_CREATE",
                {
                    "session_id": "restart-volume",
                    "program": "python3",
                    "args": [
                        "-c",
                        (
                            "import time\n"
                            "while True:\n"
                            " print('RESTART_VOLUME', flush=True)\n"
                            " time.sleep(0.01)\n"
                        ),
                    ],
                    "resource_profile": "tight",
                },
            )
            assert created["running"] is True, created
            generation = claim(socket_path, "restart-volume", OWNER)
            pane_id = created["pane_id"]

            for _ in range(300):
                assert request(socket_path, "PING", {})["daemon"] == "ok"
                inventory = request(socket_path, "CONTROL_STATUS", {})
                assert inventory["daemon"] == "ok"
                status = request(
                    socket_path,
                    "SESSION_STATUS",
                    {"session_id": "restart-volume"},
                )
                assert status["running"] is True
                snapshot = request(
                    socket_path,
                    "PANE_ATTACH",
                    {"pane_id": pane_id},
                )
                assert snapshot["pane_id"] == pane_id
                request(socket_path, "TASK_HISTORY", {})
                request(
                    socket_path,
                    "LEASE_STATUS",
                    {"session_id": "restart-volume"},
                )

            count_after_reads = idempotency_count(store_path)
            assert count_after_reads <= 2, count_after_reads
            store_size_after_reads = Path(store_path).stat().st_size
            assert store_size_after_reads < 16 * 1024 * 1024, store_size_after_reads

            for restart in range(3):
                crash_qwed(qwed)
                started = time.monotonic()
                qwed = start_qwed(args.qwed, socket_path, log_path)
                restart_ms = (time.monotonic() - started) * 1000
                assert restart_ms < 5000, restart_ms
                status = request(
                    socket_path,
                    "SESSION_STATUS",
                    {"session_id": "restart-volume"},
                )
                assert status["running"] is True, status
                assert status["pane_id"] == pane_id, status

            count_after_restarts = idempotency_count(store_path)
            assert count_after_restarts == count_after_reads, (
                count_after_reads,
                count_after_restarts,
            )
            request(
                socket_path,
                "SESSION_KILL",
                {
                    "session_id": "restart-volume",
                    "owner": OWNER,
                    "generation": generation,
                },
            )
            print(
                json.dumps(
                    {
                        "case": "SHARED-004/RG-006",
                        "read_requests_with_keys": 1800,
                        "idempotency_records": count_after_restarts,
                        "store_size_after_reads": store_size_after_reads,
                        "qwed_restarts": 3,
                        "restart_adoption": "pass",
                        "read_only_idempotency_bypass": "pass",
                    },
                    separators=(",", ":"),
                )
            )
    finally:
        stop_qwed(qwed, socket_path)


if __name__ == "__main__":
    main()
