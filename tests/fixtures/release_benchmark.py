#!/usr/bin/env python3

import argparse
import json
import math
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


FIXTURES = Path(__file__).resolve().parent
sys.path.insert(0, str(FIXTURES))
import attach_channel_probe as protocol


def percentile(samples, fraction):
    ordered = sorted(samples)
    index = max(0, math.ceil(len(ordered) * fraction) - 1)
    return ordered[index]


def wait_for_socket(path, process, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            stderr = process.stderr.read().decode(errors="replace")
            raise RuntimeError(f"qwed exited with {process.returncode}: {stderr}")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(str(path))
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for {path}")


def rss_bytes(pids):
    result = subprocess.run(
        ["ps", "-o", "rss=", "-p", ",".join(str(pid) for pid in pids)],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    kibibytes = sum(
        int(value) for value in result.stdout.split() if value.isdigit()
    )
    return kibibytes * 1024


def attach_latency(attach_path, session_id, owner):
    started = time.perf_counter()
    stream = protocol.connect(attach_path)
    try:
        protocol.send_json(
            stream,
            protocol.ATTACH_OPEN,
            protocol.open_frame(session_id, owner),
        )
        frame_type, payload = protocol.receive_frame(stream)
        assert frame_type == protocol.ATTACH_ACCEPTED, payload
        frame_type, payload = protocol.receive_frame(stream)
        assert frame_type == protocol.SNAPSHOT, payload
        snapshot = json.loads(payload)
        assert snapshot["next_stream_offset"] >= 0
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
    parser.add_argument("--qwed", type=Path, required=True)
    parser.add_argument("--sessions", type=int, default=8)
    parser.add_argument("--rounds", type=int, default=8)
    parser.add_argument("--soak-seconds", type=float, default=15)
    parser.add_argument("--max-p95-ms", type=float, default=1000)
    parser.add_argument("--max-p99-ms", type=float, default=2000)
    parser.add_argument("--max-rss-bytes", type=int, default=512 * 1024 * 1024)
    args = parser.parse_args()

    qwed = None
    worker_pids = []
    task_pids = []
    samples = []
    stream_offsets = []
    memory_samples = []
    with tempfile.TemporaryDirectory(
        prefix="qwe-release-benchmark-", dir="/tmp"
    ) as root:
        root = Path(root)
        socket_path = root / "qwe.sock"
        store_path = root / "qwe.sqlite"
        attach_path = f"{socket_path}.attach"
        qwed = subprocess.Popen(
            [
                str(args.qwed),
                "--socket",
                str(socket_path),
                "--store",
                str(store_path),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        try:
            wait_for_socket(socket_path, qwed)
            output_program = (
                "import os,time\n"
                "value=0\n"
                "while True:\n"
                " value+=1\n"
                " os.write(1, ('QWE_RELEASE_SOAK_%08d\\n' % value).encode())\n"
                " time.sleep(0.002)\n"
            )
            session_ids = []
            for index in range(args.sessions):
                session_id = f"release-soak-{index}"
                created = protocol.send_control(
                    str(socket_path),
                    "SESSION_CREATE",
                    {
                        "session_id": session_id,
                        "program": sys.executable,
                        "args": ["-c", output_program],
                    },
                    key=f"release-create-{index}",
                )
                assert created["running"] is True, created
                session_ids.append(session_id)
                worker_pids.append(created["worker_pid"])
                task_pids.append(created["pid"])

            tracked_pids = [qwed.pid, *worker_pids, *task_pids]
            memory_samples.append(rss_bytes(tracked_pids))
            for round_index in range(args.rounds):
                for session_index, session_id in enumerate(session_ids):
                    elapsed_ms, stream_offset = attach_latency(
                        attach_path,
                        session_id,
                        f"release-bench-{round_index}-{session_index}",
                    )
                    samples.append(elapsed_ms)
                    stream_offsets.append(stream_offset)
                    memory_samples.append(rss_bytes(tracked_pids))

            soak_deadline = time.monotonic() + args.soak_seconds
            soak_attaches = 0
            while time.monotonic() < soak_deadline:
                for index, session_id in enumerate(session_ids):
                    elapsed_ms, stream_offset = attach_latency(
                        attach_path,
                        session_id,
                        f"release-soak-{soak_attaches}-{index}",
                    )
                    samples.append(elapsed_ms)
                    stream_offsets.append(stream_offset)
                    soak_attaches += 1
                    memory_samples.append(rss_bytes(tracked_pids))

            assert all(worker > 0 for worker in worker_pids)
            assert max(memory_samples) > 0
            assert max(stream_offsets) > 0
        finally:
            if qwed is not None and qwed.poll() is None:
                try:
                    os.killpg(qwed.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    qwed.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    os.killpg(qwed.pid, signal.SIGKILL)
                    qwed.wait(timeout=5)

    p95 = percentile(samples, 0.95)
    p99 = percentile(samples, 0.99)
    max_rss = max(memory_samples)
    result = {
        "case": "PERF-001",
        "build_profile": "release",
        "sessions": args.sessions,
        "attach_requests": len(samples),
        "soak_seconds": args.soak_seconds,
        "p50_ms": round(percentile(samples, 0.50), 3),
        "p95_ms": round(p95, 3),
        "p99_ms": round(p99, 3),
        "max_ms": round(max(samples), 3),
        "max_rss_bytes": max_rss,
        "max_p95_ms": args.max_p95_ms,
        "max_p99_ms": args.max_p99_ms,
        "max_rss_limit_bytes": args.max_rss_bytes,
    }
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    if p95 > args.max_p95_ms or p99 > args.max_p99_ms or max_rss > args.max_rss_bytes:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
