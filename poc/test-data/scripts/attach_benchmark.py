#!/usr/bin/env python3
import argparse
import concurrent.futures
import json
import math
import subprocess
import time


def percentile(samples, fraction):
    index = max(0, math.ceil(len(samples) * fraction) - 1)
    return samples[index]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--clients", type=int, default=8)
    parser.add_argument("--iterations", type=int, default=8)
    parser.add_argument("--max-p95-ms", type=float, default=3000)
    parser.add_argument("--max-p99-ms", type=float, default=5000)
    args = parser.parse_args()

    command = [
        args.cli,
        "--socket",
        args.socket,
        "session",
        "attach",
        args.session_id,
    ]

    def attach_once(_):
        started = time.perf_counter()
        completed = subprocess.run(
            command,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        elapsed_ms = (time.perf_counter() - started) * 1000
        if completed.returncode != 0:
            return None, elapsed_ms, completed.stderr.strip()
        value = json.loads(completed.stdout)
        if value.get("session_id") != args.session_id:
            return None, elapsed_ms, "attach returned the wrong session"
        return value, elapsed_ms, None

    started = time.perf_counter()
    samples = []
    errors = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.clients) as pool:
        futures = [
            pool.submit(attach_once, index)
            for index in range(args.clients * args.iterations)
        ]
        for future in concurrent.futures.as_completed(futures):
            value, elapsed_ms, failure = future.result()
            if failure:
                errors.append(failure)
            else:
                samples.append(elapsed_ms)
                if value.get("snapshot_bytes", 0) > 1024 * 1024:
                    errors.append("attach snapshot exceeded 1 MiB")

    samples.sort()
    result = {
        "clients": args.clients,
        "iterations": args.iterations,
        "requests": len(samples) + len(errors),
        "successes": len(samples),
        "errors": errors[:10],
        "total_ms": round((time.perf_counter() - started) * 1000, 3),
        "p50_ms": round(percentile(samples, 0.50), 3) if samples else None,
        "p95_ms": round(percentile(samples, 0.95), 3) if samples else None,
        "p99_ms": round(percentile(samples, 0.99), 3) if samples else None,
        "max_ms": round(samples[-1], 3) if samples else None,
        "max_p95_ms": args.max_p95_ms,
        "max_p99_ms": args.max_p99_ms,
    }
    print(json.dumps(result, sort_keys=True))
    if (
        errors
        or not samples
        or result["p95_ms"] > args.max_p95_ms
        or result["p99_ms"] > args.max_p99_ms
    ):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
