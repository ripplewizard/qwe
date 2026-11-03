#!/usr/bin/env python3
"""Exercise process acceptance cases P01, P03-P05, P09, and P12."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import signal
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
    request_id = f"process-test-{uuid.uuid4().hex}"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(3)
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
                response = request(socket_path, "STATUS", "__process_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready")


def wait_terminal_history(
    socket_path: Path, task_id: str, timeout: float = 4
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        last = request(socket_path, "TASK_HISTORY", task_id)
        if last.get("ok") is True and last.get("instances"):
            instance = last["instances"][0]
            if instance.get("state") in {"exited", "failed"} and (
                instance.get("exit_code") is not None
            ):
                return last
        time.sleep(0.03)
    raise SmokeFailure(f"task history did not finish for {task_id}: {last!r}")


def start_task(
    socket_path: Path,
    key: str,
    log_path: Path,
    program: str,
    *arguments: str,
) -> dict[str, Any]:
    response = request(
        socket_path, "START", key, str(log_path), program, *arguments
    )
    if response.get("ok") is not True:
        raise SmokeFailure(f"START {key} failed: {response!r}")
    return response


def group_members(pgid: int) -> list[int]:
    output = subprocess.run(
        ["ps", "-eo", "pid=,pgid="],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    members = []
    for line in output.splitlines():
        fields = line.split()
        if len(fields) == 2 and int(fields[1]) == pgid:
            members.append(int(fields[0]))
    return members


def group_alive(pgid: int) -> bool:
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def create_history_database(path: Path, fake_pid: int) -> None:
    with sqlite3.connect(path) as database:
        database.executescript(
            """
            PRAGMA user_version = 2;
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                idempotency_key TEXT UNIQUE NOT NULL,
                command TEXT NOT NULL,
                log_path TEXT NOT NULL,
                pid INTEGER NOT NULL,
                pgid INTEGER NOT NULL,
                start_token TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            """
        )
        database.execute(
            """
            INSERT INTO tasks
            (id, idempotency_key, command, log_path, pid, pgid, start_token,
             state, created_at, updated_at)
            VALUES ('pid-reuse-task', 'pid-reuse-key', 'fake', 'fake.log',
                    ?, ?, 'wrong-start-token', 'running', '1', '1')
            """,
            (fake_pid, os.getpgid(fake_pid)),
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
    if result["case_id"] not in {"P01", "P03", "P04", "P05", "P09", "P12"}:
        raise SmokeFailure(f"invalid case id: {result['case_id']}")
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
    raw_dir = args.results_dir / f"process-acceptance-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    database = raw_dir / "tasks.sqlite"
    create_history_database(database, os.getpid())
    daemon_log = raw_dir / "daemon.log"
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
        "P01": {"exit_code": False, "duration": False},
        "P03": {"sigint": False, "sigterm": False, "sigkill": False},
        "P04": {"pgid_membership": False, "group_control": False},
        "P05": {"failure_exit": False, "failed_state": False},
        "P09": {"pid_reuse_rejected": False},
        "P12": {"history_after_restart": False},
    }
    metrics: dict[str, Any] = {}
    evidence: list[str] = []
    process: subprocess.Popen[str] | None = None
    log_handle: Any = None

    with tempfile.TemporaryDirectory(prefix="qwe-process.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"

        def boot() -> None:
            nonlocal process, log_handle
            log_handle = daemon_log.open("a", encoding="utf-8")
            process = subprocess.Popen(
                [str(args.binary), "daemon", str(socket_path), str(database)],
                stdout=log_handle,
                stderr=subprocess.STDOUT,
                env=os.environ.copy(),
                text=True,
            )
            wait_ready(socket_path, process)

        def shutdown() -> None:
            nonlocal process, log_handle
            if process is not None:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
                process = None
            if log_handle is not None:
                log_handle.close()
                log_handle = None

        try:
            boot()
            reused = request(socket_path, "STATUS", "pid-reuse-task")
            checks["P09"]["pid_reuse_rejected"] = (
                reused.get("ok") is True and reused.get("state") == "exited"
            )

            p01 = start_task(
                socket_path,
                "p01-key",
                raw_dir / "p01.log",
                "sh",
                "-c",
                "sleep 0.1; exit 7",
            )
            p01_history = wait_terminal_history(socket_path, p01["task_id"])
            p01_instance = p01_history["instances"][0]
            checks["P01"]["exit_code"] = p01_instance["exit_code"] == 7
            checks["P01"]["duration"] = p01_instance["duration_ms"] >= 80
            metrics["P01"] = {
                "exit_code": p01_instance["exit_code"],
                "duration_ms": p01_instance["duration_ms"],
            }

            expected = {
                "SIGINT": 130,
                "SIGTERM": 143,
                "SIGKILL": 137,
            }
            for name, expected_code in expected.items():
                task = start_task(
                    socket_path,
                    f"p03-{name.lower()}",
                    raw_dir / f"p03-{name.lower()}.log",
                    "sh",
                    "-c",
                    "sleep 30",
                )
                signaled = request(
                    socket_path, "TASK_SIGNAL", task["task_id"], name
                )
                if signaled.get("ok") is not True:
                    raise SmokeFailure(f"TASK_SIGNAL {name} failed: {signaled!r}")
                history = wait_terminal_history(socket_path, task["task_id"])
                checks["P03"][name.lower()] = (
                    history["instances"][0]["exit_code"] == expected_code
                )

            tree = start_task(
                socket_path,
                "p04-key",
                raw_dir / "p04.log",
                str(repo_root / "poc/test-data/scripts/spawn_tree.sh"),
                "3",
                "300",
            )
            deadline = time.monotonic() + 2
            members: list[int] = []
            while time.monotonic() < deadline:
                members = group_members(tree["pgid"])
                if len(members) >= 3:
                    break
                time.sleep(0.03)
            checks["P04"]["pgid_membership"] = (
                len(members) >= 3 and tree["pid"] in members
            )
            request(socket_path, "STOP", tree["task_id"])
            for _ in range(100):
                if not group_alive(tree["pgid"]):
                    checks["P04"]["group_control"] = True
                    break
                time.sleep(0.02)

            crash = start_task(
                socket_path,
                "p05-key",
                raw_dir / "p05.log",
                str(repo_root / "poc/test-data/scripts/child_crash.sh"),
                "fail",
            )
            crash_history = wait_terminal_history(socket_path, crash["task_id"])
            crash_instance = crash_history["instances"][0]
            checks["P05"]["failure_exit"] = crash_instance["exit_code"] == 42
            checks["P05"]["failed_state"] = crash_instance["state"] == "failed"

            shutdown()
            boot()
            replay = request(socket_path, "TASK_HISTORY", p01["task_id"])
            checks["P12"]["history_after_restart"] = (
                replay.get("ok") is True
                and replay.get("attempts") == 1
                and replay["instances"][0]["exit_code"] == 7
            )
        finally:
            shutdown()

    evidence = [
        str(daemon_log.relative_to(repo_root)),
        str(database.relative_to(repo_root)),
    ]
    case_files = []
    for case_id, case_checks in checks.items():
        passed = all(case_checks.values())
        result = {
            "case_id": case_id,
            "language": args.language,
            "status": "pass" if passed else "fail",
            "source_commit": source_commit,
            "environment": {
                "os": host_platform.system(),
                "architecture": host_platform.machine(),
                "toolchain": toolchain_version(args.language),
            },
            "command": f"process-acceptance-smoke {args.language} {case_id}",
            "started_at": started_iso,
            "duration_ms": round(
                (time.monotonic() - started_monotonic) * 1000
            ),
            "metrics": metrics.get(case_id, {}),
            "checks": case_checks,
            "evidence": evidence,
            "known_limitations": [
                "Shared-memory accounting is not part of this process test.",
                "macOS process start tokens have one-second resolution.",
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
