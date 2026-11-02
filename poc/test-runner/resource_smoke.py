#!/usr/bin/env python3
"""Exercise task-level CPU, memory, sampling, and GPU capability reporting."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import socket
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any


class SmokeFailure(RuntimeError):
    pass


def request(socket_path: Path, command: str, *fields: str) -> dict[str, Any]:
    request_id = f"resource-test-{uuid.uuid4().hex}"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(12)
        client.connect(str(socket_path))
        client.sendall(
            "\t".join((command, request_id, *fields)).encode() + b"\n"
        )
        response = b""
        while not response.endswith(b"\n"):
            chunk = client.recv(4096)
            if not chunk:
                raise SmokeFailure(f"{command} closed without a response")
            response += chunk
    payload = json.loads(response)
    if payload.get("request_id") != request_id:
        raise SmokeFailure(f"{command} did not echo request ID: {payload!r}")
    return payload


def wait_ready(socket_path: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise SmokeFailure("daemon exited before becoming ready")
        if socket_path.exists():
            try:
                response = request(socket_path, "STATUS", "__resource_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready")


def toolchain_version(language: str) -> str:
    commands = {
        "rust": ["rustc", "--version"],
        "go": ["go", "version"],
        "cpp": ["c++", "--version"],
    }
    try:
        result = subprocess.run(
            commands[language], check=True, capture_output=True, text=True
        )
    except (KeyError, OSError, subprocess.CalledProcessError):
        return "unavailable"
    return result.stdout.splitlines()[0] if result.stdout else "unavailable"


def validate_result(result: dict[str, Any]) -> None:
    required = {
        "case_id",
        "language",
        "status",
        "source_commit",
        "environment",
        "command",
        "started_at",
        "duration_ms",
        "metrics",
        "checks",
        "evidence",
        "known_limitations",
    }
    if set(result) != required:
        raise SmokeFailure("result fields do not match the result schema")
    if result["status"] not in {"pass", "fail", "blocked", "unsupported", "not-run"}:
        raise SmokeFailure(f"invalid status: {result['status']}")
    if not result["checks"] or not all(
        isinstance(value, bool) for value in result["checks"].values()
    ):
        raise SmokeFailure("checks must be booleans")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--language", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--results-dir", required=True, type=Path)
    args = parser.parse_args()

    started_monotonic = time.monotonic()
    started_wall = time.time()
    started_iso = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started_wall))
    run_stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(started_wall))
    run_id = f"{run_stamp}-{os.getpid()}"
    repo_root = args.binary.resolve().parents[3]
    args.results_dir.mkdir(parents=True, exist_ok=True)
    raw_dir = args.results_dir / f"resource-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    daemon_log = raw_dir / "daemon.log"
    source_commit = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "--short", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if subprocess.run(
        ["git", "-C", str(repo_root), "status", "--porcelain"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout:
        source_commit += "-dirty"

    sample_metrics: dict[int, float] = {}
    checks = {
        "RM01": {"cpu_values": False, "convention": False},
        "RM02": {"memory_values": False, "limitation": False},
        "RM03": {"intervals": False},
        "RM04": {"gpu_capability": False},
        "RM05": {"attribution_safe": False},
    }
    metrics: dict[str, Any] = {}

    with tempfile.TemporaryDirectory(prefix="qwe-resource.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"
        database = raw_dir / "tasks.sqlite"
        with daemon_log.open("w", encoding="utf-8") as log:
            process = subprocess.Popen(
                [str(args.binary), "daemon", str(socket_path), str(database)],
                stdout=log,
                stderr=subprocess.STDOUT,
                env=os.environ.copy(),
                text=True,
            )
            try:
                wait_ready(socket_path, process)
                started = request(
                    socket_path,
                    "START",
                    "resource-key",
                    str(raw_dir / "resource.log"),
                    "sh",
                    "-c",
                    "while :; do :; done",
                )
                if started.get("ok") is not True:
                    raise SmokeFailure(f"resource task failed to start: {started!r}")
                time.sleep(0.2)
                resource = request(
                    socket_path, "TASK_RESOURCE", started["task_id"]
                )
                if resource.get("ok") is not True:
                    raise SmokeFailure(f"TASK_RESOURCE failed: {resource!r}")
                checks["RM01"]["cpu_values"] = (
                    resource.get("process_count", 0) >= 1
                    and resource.get("cpu_percent", -1) >= 0
                )
                checks["RM01"]["convention"] = (
                    resource.get("cpu_convention") == "single_core_percent_sum"
                )
                checks["RM02"]["memory_values"] = (
                    resource.get("process_count", 0) >= 1
                    and resource.get("memory_bytes", 0) > 0
                )
                checks["RM02"]["limitation"] = "shared" in resource.get(
                    "memory_method", ""
                ).lower()
                gpu = resource.get("gpu", {})
                checks["RM04"]["gpu_capability"] = bool(
                    gpu.get("supported") is False
                    and gpu.get("reason")
                    or gpu.get("supported") is True
                    and isinstance(gpu.get("devices"), list)
                )
                checks["RM05"]["attribution_safe"] = (
                    gpu.get("attributed_to_task") is False
                    or gpu.get("attribution_method") == "process_pid_in_task_group"
                )
                metrics = {
                    "process_count": resource.get("process_count", 0),
                    "cpu_percent": resource.get("cpu_percent", 0),
                    "memory_bytes": resource.get("memory_bytes", 0),
                    "gpu_supported": gpu.get("supported", False),
                }

                for interval in (100, 500, 1000, 5000):
                    sample_start = time.monotonic()
                    sample = request(
                        socket_path,
                        "TASK_RESOURCE_SAMPLE",
                        started["task_id"],
                        str(interval),
                        "2",
                    )
                    elapsed_ms = (time.monotonic() - sample_start) * 1000
                    if sample.get("ok") is not True:
                        raise SmokeFailure(
                            f"resource sample {interval} failed: {sample!r}"
                        )
                    sample_metrics[interval] = round(elapsed_ms, 2)
                    if elapsed_ms < interval or elapsed_ms > interval + 3000:
                        raise SmokeFailure(
                            f"sample at {interval}ms took {elapsed_ms:.1f}ms"
                        )
                checks["RM03"]["intervals"] = set(sample_metrics) == {
                    100,
                    500,
                    1000,
                    5000,
                }
                request(socket_path, "STOP", started["task_id"])
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)

    case_files = []
    for case_id, case_checks in checks.items():
        result = {
            "case_id": case_id,
            "language": args.language,
            "status": "pass" if all(case_checks.values()) else "fail",
            "source_commit": source_commit,
            "environment": {
                "os": host_platform.system(),
                "architecture": host_platform.machine(),
                "toolchain": toolchain_version(args.language),
            },
            "command": f"resource-smoke {args.language} {case_id}",
            "started_at": started_iso,
            "duration_ms": round(
                (time.monotonic() - started_monotonic) * 1000
            ),
            "metrics": {
                **metrics,
                **{
                    f"sample_latency_{interval}_ms": value
                    for interval, value in sample_metrics.items()
                },
            },
            "checks": case_checks,
            "evidence": [
                str(daemon_log.relative_to(repo_root)),
                str(database.relative_to(repo_root)),
            ],
            "known_limitations": [
                "RSS sums may double-count shared pages.",
                "CPU is lifetime average percentage reported by ps, not an instantaneous scheduler event.",
            ],
        }
        validate_result(result)
        result_path = args.results_dir / f"{case_id}-{source_commit}-{run_id}.json"
        result_path.write_text(
            json.dumps(result, indent=2) + "\n", encoding="utf-8"
        )
        case_files.append({"case_id": case_id, "status": result["status"]})
    print(json.dumps(case_files))
    return 0 if all(item["status"] == "pass" for item in case_files) else 1


if __name__ == "__main__":
    raise SystemExit(main())
