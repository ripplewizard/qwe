#!/usr/bin/env python3
"""Exercise persisted task deadlines through the daemon socket."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import socket
import sqlite3
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any


class SmokeFailure(RuntimeError):
    pass


def request(socket_path: Path, command: str, *fields: str) -> dict[str, Any]:
    request_id = f"timeout-test-{uuid.uuid4().hex}"
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
                response = request(socket_path, "STATUS", "__timeout_ready__")
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


def wait_for_state(
    socket_path: Path, task_id: str, expected: str, timeout_seconds: float
) -> tuple[dict[str, Any], int]:
    started = time.monotonic()
    last: dict[str, Any] | None = None
    deadline = started + timeout_seconds
    while time.monotonic() < deadline:
        last = request(socket_path, "STATUS", task_id)
        if last.get("state") == expected:
            return last, round((time.monotonic() - started) * 1000)
        time.sleep(0.02)
    raise SmokeFailure(f"timed out waiting for {expected!r}: {last!r}")


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
    if result["case_id"] != "P07":
        raise SmokeFailure("result case_id must be P07")
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
    raw_dir = args.results_dir / f"P07-timeout-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    task_log = raw_dir / "task.log"
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
        "start_accepted": False,
        "deadline_observed": False,
        "state_timed_out": False,
        "process_group_stopped": False,
        "sqlite_timed_out": False,
        "output_preserved": False,
        "sigkill_escalation": False,
        "predeadline_exit_preserved": False,
        "invalid_timeout_rejected": False,
    }
    task: dict[str, Any] | None = None
    timeout_ms = 400
    deadline_started = 0.0
    deadline_elapsed_ms = 0
    sigkill_elapsed_ms = 0

    with tempfile.TemporaryDirectory(prefix="qwe-timeout.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"
        database = raw_dir / "tasks.sqlite"
        environment = os.environ.copy()
        with daemon_log.open("w", encoding="utf-8") as log:
            process = subprocess.Popen(
                [str(args.binary), "daemon", str(socket_path), str(database)],
                stdout=log,
                stderr=subprocess.STDOUT,
                env=environment,
                text=True,
            )
            try:
                wait_ready(socket_path, process)
                invalid = request(
                    socket_path,
                    "TIMEOUT_START",
                    "invalid-timeout-key",
                    str(raw_dir / "invalid.log"),
                    "250junk",
                    "sh",
                    "-c",
                    "exit 0",
                )
                checks["invalid_timeout_rejected"] = (
                    invalid.get("ok") is False
                    and invalid.get("error") == "invalid_request"
                )
                deadline_started = time.monotonic()
                started_response = request(
                    socket_path,
                    "TIMEOUT_START",
                    "deadline-key",
                    str(task_log),
                    str(timeout_ms),
                    "sh",
                    "-c",
                    "printf 'QWE_TIMEOUT_START\\n'; sleep 30",
                )
                if started_response.get("ok") is not True:
                    raise SmokeFailure(
                        f"TIMEOUT_START failed: {started_response!r}"
                    )
                task = started_response
                checks["start_accepted"] = started_response.get("state") == "running"
                task_id = started_response["task_id"]
                final, _ = wait_for_state(socket_path, task_id, "timed_out", 4)
                deadline_elapsed_ms = round(
                    (time.monotonic() - deadline_started) * 1000
                )
                checks["deadline_observed"] = (
                    timeout_ms <= deadline_elapsed_ms < 3000
                )
                checks["state_timed_out"] = final is not None and (
                    final.get("state") == "timed_out"
                )
                checks["process_group_stopped"] = not group_alive(
                    started_response["pgid"]
                )
                checks["output_preserved"] = (
                    b"QWE_TIMEOUT_START" in task_log.read_bytes()
                )

                stubborn = request(
                    socket_path,
                    "TIMEOUT_START",
                    "stubborn-key",
                    str(raw_dir / "stubborn.log"),
                    "400",
                    "sh",
                    "-c",
                    "trap '' TERM; while :; do sleep 1; done",
                )
                if stubborn.get("ok") is not True:
                    raise SmokeFailure(f"stubborn TIMEOUT_START failed: {stubborn!r}")
                stubborn_started = time.monotonic()
                wait_for_state(
                    socket_path, stubborn["task_id"], "timed_out", 4
                )
                sigkill_elapsed_ms = round(
                    (time.monotonic() - stubborn_started) * 1000
                )
                checks["sigkill_escalation"] = (
                    400 <= sigkill_elapsed_ms < 3000
                    and not group_alive(stubborn["pgid"])
                )

                fast = request(
                    socket_path,
                    "TIMEOUT_START",
                    "fast-key",
                    str(raw_dir / "fast.log"),
                    "5000",
                    "sh",
                    "-c",
                    "exit 0",
                )
                if fast.get("ok") is not True:
                    raise SmokeFailure(f"fast TIMEOUT_START failed: {fast!r}")
                fast_final, _ = wait_for_state(
                    socket_path, fast["task_id"], "exited", 3
                )
                checks["predeadline_exit_preserved"] = (
                    fast_final.get("state") == "exited"
                )
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)

        if task is not None:
            with sqlite3.connect(database) as connection:
                row = connection.execute(
                    "SELECT state FROM tasks WHERE id = ?",
                    (task["task_id"],),
                ).fetchone()
            checks["sqlite_timed_out"] = row == ("timed_out",)

    passed = all(checks.values())
    result = {
        "case_id": "P07",
        "language": args.language,
        "status": "pass" if passed else "fail",
        "source_commit": source_commit,
        "environment": {
            "os": host_platform.system(),
            "architecture": host_platform.machine(),
            "toolchain": toolchain_version(args.language),
            "environment_record": str(environment_record.relative_to(repo_root)),
        },
        "command": f"just timeout-smoke {args.language}",
        "started_at": started_iso,
        "duration_ms": round((time.monotonic() - started_monotonic) * 1000),
        "metrics": {
            "timeout_ms": timeout_ms,
            "deadline_elapsed_ms": deadline_elapsed_ms,
            "sigkill_elapsed_ms": sigkill_elapsed_ms,
        },
        "checks": checks,
        "evidence": [
            str(task_log.relative_to(repo_root)),
            str(daemon_log.relative_to(repo_root)),
            str(environment_record.relative_to(repo_root)),
        ],
        "known_limitations": [
            "The POC deadline watcher is in memory and is not recovered after daemon restart.",
            "Graceful termination uses a fixed short grace period.",
        ],
    }
    validate_result_document(result)
    result_path = args.results_dir / f"P07-{source_commit}-{run_id}.json"
    result_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "checks": checks}))
    return 0 if passed else 1


def group_alive(pgid: int) -> bool:
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


if __name__ == "__main__":
    raise SystemExit(main())
