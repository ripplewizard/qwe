#!/usr/bin/env python3
"""Exercise additive SQLite schema migration and daemon version safety."""

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
    request_id = f"upgrade-test-{uuid.uuid4().hex}"
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
                response = request(socket_path, "STATUS", "__upgrade_ready__")
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
    if result["case_id"] != "D06":
        raise SmokeFailure("result case_id must be D06")
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


def create_legacy_database(path: Path, version: int, task_id: str) -> None:
    with sqlite3.connect(path) as database:
        database.executescript(
            """
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
            VALUES (?, ?, 'legacy command', 'legacy.log',
                    999999, 999999, 'legacy-token', 'stopped', '1', '1')
            """,
            (task_id, f"{task_id}-key"),
        )
        database.execute(f"PRAGMA user_version = {version}")


def start_daemon(
    binary: Path,
    socket_path: Path,
    database: Path,
    log_path: Path,
    append: bool,
) -> tuple[subprocess.Popen[str], Any]:
    log = log_path.open("a" if append else "w", encoding="utf-8")
    process = subprocess.Popen(
        [str(binary), "daemon", str(socket_path), str(database)],
        stdout=log,
        stderr=subprocess.STDOUT,
        env=os.environ.copy(),
        text=True,
    )
    return process, log


def stop_daemon(process: subprocess.Popen[str], log: Any) -> None:
    process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=2)
    log.close()


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
    raw_dir = args.results_dir / f"D06-upgrade-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    database = raw_dir / "legacy.sqlite"
    daemon_log = raw_dir / "daemon.log"
    future_log = raw_dir / "future-daemon.log"
    environment_record = raw_dir / "environment.txt"
    create_legacy_database(database, 0, "legacy-task")
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
        "legacy_opens": False,
        "task_preserved": False,
        "schema_upgraded": False,
        "new_tables_created": False,
        "migration_idempotent": False,
        "schema_v1_upgraded": False,
        "v1_task_preserved": False,
        "daemon_restart_compatible": False,
        "future_schema_rejected": False,
    }
    schema_version = 0

    with tempfile.TemporaryDirectory(prefix="qwe-upgrade.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"
        process, log = start_daemon(
            args.binary, socket_path, database, daemon_log, append=False
        )
        try:
            wait_ready(socket_path, process)
            checks["legacy_opens"] = True
            legacy = request(socket_path, "STATUS", "legacy-task")
            checks["task_preserved"] = (
                legacy.get("ok") is True
                and legacy.get("task_id") == "legacy-task"
                and legacy.get("state") == "stopped"
            )
        finally:
            stop_daemon(process, log)

        with sqlite3.connect(database) as connection:
            schema_version = connection.execute("PRAGMA user_version").fetchone()[0]
            tables = {
                row[0]
                for row in connection.execute(
                    "SELECT name FROM sqlite_master WHERE type='table'"
                )
            }
        checks["schema_upgraded"] = schema_version == 2
        checks["new_tables_created"] = {
            "restart_tasks",
            "task_instances",
        }.issubset(tables)

        process, log = start_daemon(
            args.binary, socket_path, database, daemon_log, append=True
        )
        try:
            wait_ready(socket_path, process)
            restarted = request(socket_path, "STATUS", "legacy-task")
            checks["daemon_restart_compatible"] = (
                restarted.get("ok") is True
                and restarted.get("state") == "stopped"
            )
        finally:
            stop_daemon(process, log)
        with sqlite3.connect(database) as connection:
            checks["migration_idempotent"] = (
                connection.execute("PRAGMA user_version").fetchone()[0] == 2
            )

        v1_database = raw_dir / "version-one.sqlite"
        create_legacy_database(v1_database, 1, "v1-task")
        v1_socket = Path(temp_name) / "v1.sock"
        process, log = start_daemon(
            args.binary, v1_socket, v1_database, daemon_log, append=True
        )
        try:
            wait_ready(v1_socket, process)
            v1_status = request(v1_socket, "STATUS", "v1-task")
            checks["v1_task_preserved"] = (
                v1_status.get("ok") is True
                and v1_status.get("state") == "stopped"
            )
        finally:
            stop_daemon(process, log)
        with sqlite3.connect(v1_database) as connection:
            checks["schema_v1_upgraded"] = (
                connection.execute("PRAGMA user_version").fetchone()[0] == 2
            )

        future_database = raw_dir / "future.sqlite"
        with sqlite3.connect(future_database) as connection:
            connection.execute("PRAGMA user_version = 999")
        future_socket = Path(temp_name) / "future.sock"
        try:
            future = subprocess.run(
                [str(args.binary), "daemon", str(future_socket), str(future_database)],
                capture_output=True,
                text=True,
                timeout=3,
                env=os.environ.copy(),
            )
            future_log.write_text(
                future.stdout + future.stderr, encoding="utf-8"
            )
            output = future.stdout + future.stderr
            checks["future_schema_rejected"] = (
                future.returncode != 0
                and "newer than supported" in output
                and not future_socket.exists()
            )
        except subprocess.TimeoutExpired as error:
            future_log.write_text(
                (error.stdout or "") + (error.stderr or ""), encoding="utf-8"
            )

    passed = all(checks.values())
    result = {
        "case_id": "D06",
        "language": args.language,
        "status": "pass" if passed else "fail",
        "source_commit": source_commit,
        "environment": {
            "os": host_platform.system(),
            "architecture": host_platform.machine(),
            "toolchain": toolchain_version(args.language),
            "environment_record": str(environment_record.relative_to(repo_root)),
        },
        "command": f"just upgrade-smoke {args.language}",
        "started_at": started_iso,
        "duration_ms": round((time.monotonic() - started_monotonic) * 1000),
        "metrics": {"schema_version": schema_version},
        "checks": checks,
        "evidence": [
            str(daemon_log.relative_to(repo_root)),
            str(future_log.relative_to(repo_root)),
            str(environment_record.relative_to(repo_root)),
            str(database.relative_to(repo_root)),
            str(v1_database.relative_to(repo_root)),
        ],
        "known_limitations": [
            "Online upgrade while another daemon owns the database is not covered.",
            "Only additive schema migrations are implemented in this POC slice.",
        ],
    }
    validate_result_document(result)
    result_path = args.results_dir / f"D06-{source_commit}-{run_id}.json"
    result_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "checks": checks}))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
