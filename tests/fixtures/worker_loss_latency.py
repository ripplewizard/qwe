#!/usr/bin/env python3

import argparse
import json
import math
import os
import signal
import socket
import struct
import time


PROTOCOL_VERSION = 1


def receive_exact(stream, count):
    chunks = []
    remaining = count
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise EOFError("qwed closed during a response")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def request(socket_path, operation, payload, sequence):
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stream.settimeout(5)
    stream.connect(socket_path)
    envelope = {
        "version": PROTOCOL_VERSION,
        "request_id": f"latency-{sequence}",
        "idempotency_key": f"latency-key-{os.getpid()}-{sequence}",
        "operation": operation,
        "payload": payload,
    }
    body = json.dumps(envelope, separators=(",", ":")).encode()
    stream.sendall(struct.pack(">I", len(body)))
    stream.sendall(body)
    length = struct.unpack(">I", receive_exact(stream, 4))[0]
    response = json.loads(receive_exact(stream, length))
    stream.close()
    result = response["result"]
    if "Err" in result:
        raise RuntimeError(result["Err"])
    return result["Ok"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--max-p95-ms", type=float, default=50.0)
    args = parser.parse_args()

    latencies = []
    sequence = 0
    for index in range(args.samples):
        session_id = f"latency-{os.getpid()}-{index}"
        sequence += 1
        created = request(
            args.socket,
            "SESSION_CREATE",
            {"session_id": session_id, "program": "sh", "args": []},
            sequence,
        )
        worker_pid = created["worker_pid"]
        started = time.perf_counter()
        os.kill(worker_pid, signal.SIGKILL)
        deadline = started + 1.0
        while time.perf_counter() < deadline:
            sequence += 1
            status = request(args.socket, "CONTROL_STATUS", {}, sequence)
            if any(
                session["id"] == session_id and session["state"] == "lost"
                for session in status["sessions"]
            ):
                latencies.append((time.perf_counter() - started) * 1000)
                break
            time.sleep(0.002)
        else:
            raise RuntimeError(f"worker loss was not reconciled for {session_id}")

    ordered = sorted(latencies)
    p95_index = max(0, math.ceil(len(ordered) * 0.95) - 1)
    p95 = ordered[p95_index]
    result = {
        "case": "LIVE-011",
        "samples": len(latencies),
        "p50_ms": ordered[len(ordered) // 2],
        "p95_ms": p95,
        "max_ms": ordered[-1],
        "max_p95_ms": args.max_p95_ms,
    }
    print(json.dumps(result, separators=(",", ":")))
    raise SystemExit(0 if p95 <= args.max_p95_ms else 1)


if __name__ == "__main__":
    main()
