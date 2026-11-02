#!/usr/bin/env python3
"""Exercise D01, D02, D04, D05, and D08 through real daemon paths."""

from __future__ import annotations

import argparse
import errno
import json
import os
import platform as host_platform
import shutil
import socket
import sqlite3
import statistics
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path
from typing import Any


class SmokeFailure(RuntimeError):
    pass


def raw_request(socket_path: Path, line: str) -> dict[str, Any]:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(8)
        client.connect(str(socket_path))
        client.sendall(line.encode() + b"\n")
        response = b""
        while not response.endswith(b"\n"):
            chunk = client.recv(4096)
            if not chunk:
                raise SmokeFailure("daemon closed without a response")
            response += chunk
    return json.loads(response)


def request(socket_path: Path, command: str, *fields: str) -> dict[str, Any]:
    request_id = f"daemon-test-{uuid.uuid4().hex}"
    response = raw_request(
        socket_path, "\t".join((command, request_id, *fields))
    )
    if response.get("request_id") != request_id:
        raise SmokeFailure(f"{command} did not echo request ID: {response!r}")
    return response


def wait_ready(socket_path: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise SmokeFailure("daemon exited before becoming ready")
        if socket_path.exists():
            try:
                response = request(socket_path, "STATUS", "__ready__")
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


def rss_kib(pid: int) -> int:
    output = subprocess.run(
        ["ps", "-o", "rss=", "-p", str(pid)],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    return int(output or 0)


def run_checked(command: list[str]) -> str:
    result = subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def mount_isolated_filesystem(root: Path) -> tuple[Path, Path, str]:
    root.mkdir(parents=True, exist_ok=True)
    image = root / (
        "disk.dmg" if host_platform.system() == "Darwin" else "disk.img"
    )
    mountpoint = root / "mnt"
    mountpoint.mkdir()
    if host_platform.system() == "Linux":
        for tool in ("truncate", "mkfs.ext4", "fuse2fs", "fusermount3"):
            if shutil.which(tool) is None:
                raise SmokeFailure(f"required disk-full tool is missing: {tool}")
        run_checked(["truncate", "-s", "16M", str(image)])
        run_checked(["mkfs.ext4", "-q", "-F", str(image)])
        run_checked(["fuse2fs", "-o", "fakeroot", str(image), str(mountpoint)])
        return image, mountpoint, "fusermount3"
    if host_platform.system() == "Darwin":
        if shutil.which("hdiutil") is None:
            raise SmokeFailure("required disk-full tool is missing: hdiutil")
        run_checked(
            [
                "hdiutil",
                "create",
                "-quiet",
                "-size",
                "32m",
                "-fs",
                "HFS+",
                "-volname",
                "QWE_D04",
                str(image),
            ]
        )
        run_checked(
            [
                "hdiutil",
                "attach",
                "-nobrowse",
                "-mountpoint",
                str(mountpoint),
                str(image),
            ]
        )
        return image, mountpoint, "hdiutil"
    raise SmokeFailure(
        f"isolated disk-full injection is unsupported on {host_platform.system()}"
    )


def unmount_isolated_filesystem(mountpoint: Path, tool: str) -> None:
    if tool == "fusermount3":
        subprocess.run(
            ["fusermount3", "-u", str(mountpoint)],
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
    elif tool == "hdiutil":
        subprocess.run(
            ["hdiutil", "detach", "-quiet", str(mountpoint)],
            check=False,
            capture_output=True,
            text=True,
            timeout=20,
        )


def disk_full_injection(
    binary: Path,
    root: Path,
    source_commit: str,
    evidence: Path,
    repo_root: Path,
) -> tuple[bool, dict[str, Any]]:
    image, mountpoint, unmount_tool = mount_isolated_filesystem(root)
    socket_path = root / "disk-full.sock"
    database = mountpoint / "tasks.sqlite"
    daemon_log = evidence.with_suffix(".daemon.log")
    metrics: dict[str, Any] = {}
    passed = False
    try:
        before_usage = shutil.disk_usage(mountpoint)
        with daemon_log.open("w", encoding="utf-8") as log:
            daemon = subprocess.Popen(
                [str(binary), "daemon", str(socket_path), str(database)],
                stdout=log,
                stderr=subprocess.STDOUT,
                env=os.environ.copy(),
                text=True,
            )
            try:
                wait_ready(socket_path, daemon)
                initial = request(
                    socket_path,
                    "START",
                    "disk-full-initial",
                    str(mountpoint / "task.log"),
                    "sh",
                    "-c",
                    "sleep 20",
                )
                if initial.get("ok") is not True:
                    raise SmokeFailure(
                        f"initial task before disk fill failed: {initial!r}"
                    )

                checkpoint = sqlite3.connect(database, timeout=5)
                checkpoint.execute("PRAGMA wal_checkpoint(TRUNCATE)")
                checkpoint.close()
                rejected_logs = []
                for index in range(24):
                    log_path = mountpoint / f"rejected-{index}.log"
                    log_path.touch()
                    rejected_logs.append(log_path)
                filler = mountpoint / "fill.bin"
                written = 0
                with filler.open("wb", buffering=0) as stream:
                    chunk = b"Q" * (64 * 1024)
                    while True:
                        try:
                            count = stream.write(chunk)
                        except OSError as error:
                            if error.errno == errno.ENOSPC:
                                break
                            raise
                        if count == 0:
                            raise SmokeFailure(
                                "disk filler wrote zero bytes without ENOSPC"
                            )
                        written += count
                    os.fsync(stream.fileno())
                after_full_usage = shutil.disk_usage(mountpoint)
                failed_start = None
                successful_ids = []
                for index, log_path in enumerate(rejected_logs):
                    candidate = request(
                        socket_path,
                        "START",
                        f"disk-full-rejected-{index}",
                        str(log_path),
                        "sh",
                        "-c",
                        "exit 0",
                    )
                    if candidate.get("ok") is False:
                        failed_start = candidate
                        break
                    successful_ids.append(candidate["task_id"])
                status_while_full = request(
                    socket_path, "STATUS", initial["task_id"]
                )
                filler.unlink()
                after_cleanup_usage = shutil.disk_usage(mountpoint)
                recovered_start = request(
                    socket_path,
                    "START",
                    "disk-full-recovered",
                    str(mountpoint / "recovered.log"),
                    "sh",
                    "-c",
                    "exit 0",
                )
                request(socket_path, "STOP", initial["task_id"])
                for task_id in successful_ids:
                    request(socket_path, "STOP", task_id)
                passed = (
                    failed_start is not None
                    and failed_start.get("error") in {"internal", "busy", "full"}
                    and status_while_full.get("ok") is True
                    and recovered_start.get("ok") is True
                    and after_full_usage.free < before_usage.free
                )
                metrics = {
                    "image_bytes": image.stat().st_size,
                    "free_before_bytes": before_usage.free,
                    "free_at_enospc_bytes": after_full_usage.free,
                    "free_after_cleanup_bytes": after_cleanup_usage.free,
                    "filler_bytes": written,
                    "write_error": "ENOSPC",
                    "write_attempts_before_failure": (
                        len(successful_ids) + (1 if failed_start else 0)
                    ),
                    "failed_start_error": failed_start.get("error")
                    if failed_start
                    else None,
                    "read_during_full": status_while_full.get("ok") is True,
                    "start_after_cleanup": recovered_start.get("ok") is True,
                }
            finally:
                daemon.terminate()
                try:
                    daemon.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait(timeout=2)
    finally:
        unmount_isolated_filesystem(mountpoint, unmount_tool)
    evidence.write_text(
        json.dumps(
            {
                "source_commit": source_commit,
                "platform": host_platform.platform(),
                "metrics": metrics,
                "daemon_log": str(daemon_log.relative_to(repo_root)),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return passed, metrics


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
    if set(result) != required or not all(
        isinstance(value, bool) for value in result["checks"].values()
    ):
        raise SmokeFailure("invalid result document")


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
    raw_dir = args.results_dir / f"daemon-depth-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    database = raw_dir / "tasks.sqlite"
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

    checks = {
        "D01": {"request_ids": False, "error_codes": False},
        "D02": {"concurrent_state": False, "latency_recorded": False},
        "D04": {
            "corrupt_rejected": False,
            "lock_contention_explicit": False,
            "disk_full_isolated": False,
        },
        "D05": {"partial_disconnect": False, "no_stuck_request": False},
        "D08": {"sustained_requests": False, "rss_recorded": False},
    }
    metrics: dict[str, Any] = {}

    with tempfile.TemporaryDirectory(prefix="qwe-daemon-depth.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"
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
                task = request(
                    socket_path,
                    "START",
                    "depth-key",
                    str(raw_dir / "task.log"),
                    "sh",
                    "-c",
                    "sleep 30",
                )
                if task.get("ok") is not True:
                    raise SmokeFailure(f"task start failed: {task!r}")

                unknown_id = f"id-{uuid.uuid4().hex}"
                unknown = raw_request(
                    socket_path, f"NOT_A_COMMAND\t{unknown_id}"
                )
                missing_id = f"id-{uuid.uuid4().hex}"
                missing = raw_request(
                    socket_path, f"STATUS\t{missing_id}\tmissing-task"
                )
                checks["D01"]["request_ids"] = (
                    unknown.get("request_id") == unknown_id
                    and missing.get("request_id") == missing_id
                )
                checks["D01"]["error_codes"] = (
                    unknown.get("error") == "invalid_request"
                    and missing.get("error") == "not_found"
                )

                latencies: list[float] = []
                errors = 0
                state_ok = True

                def query() -> None:
                    nonlocal errors, state_ok
                    started = time.monotonic()
                    try:
                        response = request(
                            socket_path, "STATUS", task["task_id"]
                        )
                        if response.get("ok") is not True or response.get(
                            "task_id"
                        ) != task["task_id"]:
                            errors += 1
                            state_ok = False
                    except Exception:
                        errors += 1
                    latencies.append((time.monotonic() - started) * 1000)

                threads = [threading.Thread(target=query) for _ in range(40)]
                for thread in threads:
                    thread.start()
                for thread in threads:
                    thread.join()
                ordered = sorted(latencies)
                p95 = ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))]
                checks["D02"]["concurrent_state"] = errors == 0 and state_ok
                checks["D02"]["latency_recorded"] = len(latencies) == 40
                metrics["D02"] = {
                    "requests": len(latencies),
                    "median_ms": round(statistics.median(latencies), 3),
                    "p95_ms": round(p95, 3),
                }

                with socket.socket(
                    socket.AF_UNIX, socket.SOCK_STREAM
                ) as partial:
                    partial.connect(str(socket_path))
                    partial.sendall(
                        f"STATUS\tpartial-{uuid.uuid4().hex}".encode()
                    )
                checks["D05"]["partial_disconnect"] = True
                stuck_started = time.monotonic()
                after_partial = request(
                    socket_path, "STATUS", task["task_id"]
                )
                checks["D05"]["no_stuck_request"] = (
                    after_partial.get("ok") is True
                    and (time.monotonic() - stuck_started) < 1
                )

                rss_before = rss_kib(process.pid)
                sustained_errors = 0
                sustained_started = time.monotonic()
                count = 0
                while time.monotonic() - sustained_started < 8:
                    response = request(socket_path, "STATUS", task["task_id"])
                    if response.get("ok") is not True:
                        sustained_errors += 1
                    count += 1
                    time.sleep(0.05)
                rss_after = rss_kib(process.pid)
                checks["D08"]["sustained_requests"] = (
                    count >= 100 and sustained_errors == 0
                )
                checks["D08"]["rss_recorded"] = rss_before > 0 and rss_after > 0
                metrics["D08"] = {
                    "duration_seconds": 8,
                    "requests": count,
                    "errors": sustained_errors,
                    "rss_before_kib": rss_before,
                    "rss_after_kib": rss_after,
                }

                lock_connection = sqlite3.connect(database, timeout=1)
                lock_connection.execute("BEGIN EXCLUSIVE")
                lock_started = time.monotonic()
                locked = request(
                    socket_path,
                    "START",
                    "locked-key",
                    str(raw_dir / "locked.log"),
                    "sh",
                    "-c",
                    "exit 0",
                )
                lock_latency = (time.monotonic() - lock_started) * 1000
                lock_connection.rollback()
                lock_connection.close()
                checks["D04"]["lock_contention_explicit"] = (
                    locked.get("ok") is False
                    and locked.get("error") in {"internal", "busy"}
                )
                metrics["D04"] = {"lock_latency_ms": round(lock_latency, 3)}
                request(socket_path, "STOP", task["task_id"])
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)

        corrupt = raw_dir / "corrupt.sqlite"
        corrupt.write_text("not sqlite", encoding="utf-8")
        corrupt_socket = Path(temp_name) / "corrupt.sock"
        corrupt_result = subprocess.run(
            [str(args.binary), "daemon", str(corrupt_socket), str(corrupt)],
            capture_output=True,
            text=True,
            timeout=3,
            env=os.environ.copy(),
        )
        checks["D04"]["corrupt_rejected"] = corrupt_result.returncode != 0

        disk_evidence = raw_dir / "disk-full.json"
        try:
            disk_full_ok, disk_metrics = disk_full_injection(
                args.binary,
                Path(temp_name) / "disk-full",
                source_commit,
                disk_evidence,
                repo_root,
            )
        except (OSError, SmokeFailure, subprocess.SubprocessError) as error:
            disk_full_ok = False
            disk_metrics = {"error": str(error)}
            disk_evidence.write_text(
                json.dumps(
                    {
                        "source_commit": source_commit,
                        "platform": host_platform.platform(),
                        "metrics": disk_metrics,
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
        checks["D04"]["disk_full_isolated"] = disk_full_ok
        metrics.setdefault("D04", {}).update(disk_metrics)

    case_files = []
    for case_id, case_checks in checks.items():
        status = "pass" if all(case_checks.values()) else "fail"
        result = {
            "case_id": case_id,
            "language": args.language,
            "status": status,
            "source_commit": source_commit,
            "environment": {
                "os": host_platform.system(),
                "architecture": host_platform.machine(),
                "toolchain": toolchain_version(args.language),
            },
            "command": f"daemon-depth-smoke {args.language} {case_id}",
            "started_at": started_iso,
            "duration_ms": round(
                (time.monotonic() - started_monotonic) * 1000
            ),
            "metrics": metrics.get(case_id, {}),
            "checks": case_checks,
            "evidence": [
                str(path.relative_to(repo_root))
                for path in (
                    daemon_log,
                    raw_dir / "disk-full.json",
                    raw_dir / "disk-full.daemon.log",
                )
                if path.exists()
            ],
            "known_limitations": [
                "Full-disk injection uses a temporary isolated filesystem and a real SQLite database."
                if case_id == "D04"
                else "POC request protocol is not a production wire contract.",
                "POC request protocol is not a production wire contract.",
            ],
        }
        validate_result(result)
        result_path = args.results_dir / f"{case_id}-{source_commit}-{run_id}.json"
        result_path.write_text(
            json.dumps(result, indent=2) + "\n", encoding="utf-8"
        )
        case_files.append({"case_id": case_id, "status": status})
    print(json.dumps(case_files))
    return 0 if all(item["status"] in {"pass", "blocked"} for item in case_files) else 1


if __name__ == "__main__":
    raise SystemExit(main())
