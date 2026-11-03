#!/usr/bin/env python3
"""Exercise restart policies and independent TaskInstance history."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path
from typing import Any


class SmokeFailure(RuntimeError):
    pass


def request(socket_path: Path, command: str, *fields: str) -> dict[str, Any]:
    request_id = f"restart-test-{uuid.uuid4().hex}"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(2)
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
                response = request(socket_path, "STATUS", "__restart_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready")


def wait_completed(
    socket_path: Path, key: str, attempts: int
) -> dict[str, Any]:
    deadline = time.monotonic() + 6
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        last = request(socket_path, "RESTART_STATUS", key)
        if last.get("ok") is True and last.get("state") == "completed":
            if len(last.get("instances", [])) == attempts:
                return last
        time.sleep(0.03)
    raise SmokeFailure(f"restart task {key} did not complete: {last!r}")


def wait_restart_state(
    socket_path: Path, key: str, expected: str, timeout_seconds: float = 4
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        last = request(socket_path, "RESTART_STATUS", key)
        if last.get("ok") is True and last.get("state") == expected:
            return last
        time.sleep(0.03)
    raise SmokeFailure(
        f"restart task {key} did not reach {expected}: {last!r}"
    )


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


def validate_result_document(result: dict[str, Any]) -> None:
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
        raise SmokeFailure(
            f"result fields differ from schema: {sorted(set(result) ^ required)}"
        )
    if result["case_id"] != "P10":
        raise SmokeFailure("result case_id must be P10")
    if result["language"] not in {"rust", "go", "cpp"}:
        raise SmokeFailure(f"invalid result language: {result['language']!r}")
    if result["status"] not in {"pass", "fail", "blocked", "unsupported", "not-run"}:
        raise SmokeFailure(f"invalid result status: {result['status']!r}")
    if not isinstance(result["duration_ms"], (int, float)) or result["duration_ms"] < 0:
        raise SmokeFailure("result duration_ms must be a non-negative number")
    if not result["checks"] or not all(
        isinstance(value, bool) for value in result["checks"].values()
    ):
        raise SmokeFailure("result checks must be a non-empty boolean map")
    if not result["evidence"] or not all(
        isinstance(value, str) for value in result["evidence"]
    ):
        raise SmokeFailure("result evidence must be a non-empty string list")


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
    raw_dir = args.results_dir / f"P10-restart-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    daemon_log = raw_dir / "daemon.log"
    environment_record = raw_dir / "environment.txt"
    environment_record.write_text(
        "\n".join(
            [
                f"os={host_platform.system()}",
                f"release={host_platform.release()}",
                f"architecture={host_platform.machine()}",
                f"platform={args.platform}",
                f"language={args.language}",
                f"toolchain={toolchain_version(args.language)}",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    source_commit = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "--short", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if subprocess.run(
        [
            "git",
            "-C",
            str(repo_root),
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout:
        source_commit += "-dirty"

    checks = {
        "never_policy": False,
        "on_failure_policy": False,
        "always_policy": False,
        "independent_instances": False,
        "exit_codes_persisted": False,
        "idempotent_start": False,
        "sqlite_history": False,
        "invalid_policy_rejected": False,
        "invalid_max_attempts_rejected": False,
        "concurrent_start_idempotent": False,
        "signal_exit_code": False,
        "spawn_failure_reported": False,
    }
    results: dict[str, dict[str, Any]] = {}

    with tempfile.TemporaryDirectory(prefix="qwe-restart.") as temp_name:
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
                invalid_policy = request(
                    socket_path,
                    "RESTART_START",
                    "invalid-policy-key",
                    str(raw_dir / "invalid-policy"),
                    "sometimes",
                    "1",
                    "sh",
                    "-c",
                    "exit 0",
                )
                checks["invalid_policy_rejected"] = (
                    invalid_policy.get("ok") is False
                    and invalid_policy.get("error") == "invalid_request"
                )
                invalid_attempts = request(
                    socket_path,
                    "RESTART_START",
                    "invalid-attempts-key",
                    str(raw_dir / "invalid-attempts"),
                    "never",
                    "0",
                    "sh",
                    "-c",
                    "exit 0",
                )
                checks["invalid_max_attempts_rejected"] = (
                    invalid_attempts.get("ok") is False
                    and invalid_attempts.get("error") == "invalid_request"
                )

                spawn_failure = request(
                    socket_path,
                    "RESTART_START",
                    "missing-program-key",
                    str(raw_dir / "missing"),
                    "never",
                    "1",
                    "/definitely/missing/qwe-program",
                )
                if spawn_failure.get("ok") is not True:
                    raise SmokeFailure(
                        f"missing-program RESTART_START failed: {spawn_failure!r}"
                    )
                failed = wait_restart_state(
                    socket_path, "missing-program-key", "failed"
                )
                checks["spawn_failure_reported"] = (
                    failed.get("state") == "failed"
                    and failed.get("attempts") == 0
                )

                concurrent_responses: list[dict[str, Any]] = []

                def concurrent_start() -> None:
                    concurrent_responses.append(
                        request(
                            socket_path,
                            "RESTART_START",
                            "concurrent-key",
                            str(raw_dir / "concurrent"),
                            "never",
                            "1",
                            "sh",
                            "-c",
                            "sleep 0.2; exit 0",
                        )
                    )

                concurrent_threads = [
                    threading.Thread(target=concurrent_start) for _ in range(2)
                ]
                for thread in concurrent_threads:
                    thread.start()
                for thread in concurrent_threads:
                    thread.join()
                concurrent_result = wait_completed(
                    socket_path, "concurrent-key", 1
                )
                checks["concurrent_start_idempotent"] = (
                    sum(response.get("created") is True for response in concurrent_responses)
                    == 1
                    and len(concurrent_result["instances"]) == 1
                )

                common = ["sh", "-c", "exit 0"]
                requests = {
                    "never": request(
                        socket_path,
                        "RESTART_START",
                        "never-key",
                        str(raw_dir / "never"),
                        "never",
                        "3",
                        *common,
                    ),
                    "on-failure": request(
                        socket_path,
                        "RESTART_START",
                        "failure-key",
                        str(raw_dir / "failure"),
                        "on-failure",
                        "3",
                        "sh",
                        "-c",
                        'if [ ! -f "$1" ]; then : > "$1"; exit 7; fi; exit 0',
                        "sh",
                        str(raw_dir / "failure-state"),
                    ),
                    "always": request(
                        socket_path,
                        "RESTART_START",
                        "always-key",
                        str(raw_dir / "always"),
                        "always",
                        "3",
                        *common,
                    ),
                }
                for key, response in requests.items():
                    if response.get("ok") is not True:
                        raise SmokeFailure(f"{key} RESTART_START failed: {response!r}")

                results["never"] = wait_completed(socket_path, "never-key", 1)
                results["on-failure"] = wait_completed(
                    socket_path, "failure-key", 2
                )
                results["always"] = wait_completed(
                    socket_path, "always-key", 3
                )
                signal_result = request(
                    socket_path,
                    "RESTART_START",
                    "signal-key",
                    str(raw_dir / "signal"),
                    "never",
                    "1",
                    "sh",
                    "-c",
                    "kill -TERM $$",
                )
                if signal_result.get("ok") is not True:
                    raise SmokeFailure(
                        f"signal RESTART_START failed: {signal_result!r}"
                    )
                signal_final = wait_completed(socket_path, "signal-key", 1)
                checks["signal_exit_code"] = (
                    signal_final["instances"][0]["exit_code"] == 143
                )

                never_codes = [item["exit_code"] for item in results["never"]["instances"]]
                failure_codes = [
                    item["exit_code"] for item in results["on-failure"]["instances"]
                ]
                always_codes = [
                    item["exit_code"] for item in results["always"]["instances"]
                ]
                checks["never_policy"] = never_codes == [0]
                checks["on_failure_policy"] = failure_codes == [7, 0]
                checks["always_policy"] = always_codes == [0, 0, 0]
                checks["independent_instances"] = all(
                    len(result["instances"]) == len(
                        {item["attempt"] for item in result["instances"]}
                    )
                    for result in results.values()
                )
                checks["exit_codes_persisted"] = (
                    checks["never_policy"]
                    and checks["on_failure_policy"]
                    and checks["always_policy"]
                )

                duplicate = request(
                    socket_path,
                    "RESTART_START",
                    "never-key",
                    str(raw_dir / "never-again"),
                    "always",
                    "5",
                    *common,
                )
                checks["idempotent_start"] = (
                    duplicate.get("created") is False
                    and len(duplicate.get("instances", [])) == 1
                )
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)

        with sqlite3.connect(database) as connection:
            count = connection.execute(
                "SELECT COUNT(*) FROM task_instances"
            ).fetchone()[0]
            partitions = dict(
                connection.execute(
                    "SELECT task_key, COUNT(*) FROM task_instances GROUP BY task_key"
                ).fetchall()
            )
        checks["sqlite_history"] = count == 8 and partitions == {
            "never-key": 1,
            "failure-key": 2,
            "always-key": 3,
            "concurrent-key": 1,
            "signal-key": 1,
        }

    passed = all(checks.values())
    result = {
        "case_id": "P10",
        "language": args.language,
        "status": "pass" if passed else "fail",
        "source_commit": source_commit,
        "environment": {
            "os": host_platform.system(),
            "architecture": host_platform.machine(),
            "toolchain": toolchain_version(args.language),
            "environment_record": str(environment_record.relative_to(repo_root)),
        },
        "command": f"just restart-smoke {args.language}",
        "started_at": started_iso,
        "duration_ms": round((time.monotonic() - started_monotonic) * 1000),
        "metrics": {
            "never_attempts": len(results.get("never", {}).get("instances", [])),
            "on_failure_attempts": len(
                results.get("on-failure", {}).get("instances", [])
            ),
            "always_attempts": len(
                results.get("always", {}).get("instances", [])
            ),
        },
        "checks": checks,
        "evidence": [
            str(daemon_log.relative_to(repo_root)),
            str(environment_record.relative_to(repo_root)),
            *[
                str(path.relative_to(repo_root))
                for path in sorted(raw_dir.glob("*.log"))
            ],
        ],
        "known_limitations": [
            "In-progress restart sequences are not recovered after daemon restart.",
            "The POC has no configurable delay or backoff between attempts.",
        ],
    }
    validate_result_document(result)
    result_path = args.results_dir / f"P10-{source_commit}-{run_id}.json"
    result_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "checks": checks}))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
