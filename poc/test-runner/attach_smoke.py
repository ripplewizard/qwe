#!/usr/bin/env python3
"""Exercise T08 attach, detach, snapshot, and input continuity."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import socket
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
    request_id = f"attach-test-{uuid.uuid4().hex}"
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


def success(response: dict[str, Any], operation: str) -> dict[str, Any]:
    if response.get("ok") is not True:
        raise SmokeFailure(f"{operation} failed: {response!r}")
    return response


def wait_ready(socket_path: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise SmokeFailure("daemon exited before becoming ready")
        if socket_path.exists():
            try:
                response = request(socket_path, "STATUS", "__attach_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready")


def snapshot_text(response: dict[str, Any]) -> bytes:
    try:
        return bytes.fromhex(response["snapshot_hex"])
    except (KeyError, ValueError) as error:
        raise SmokeFailure(f"invalid snapshot response: {response!r}") from error


def wait_for_text(
    socket_path: Path,
    session_id: str,
    marker: bytes,
    *,
    running: bool | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + 3
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        last = success(
            request(socket_path, "SESSION_STATUS", session_id),
            "SESSION_STATUS",
        )
        if marker in snapshot_text(last) and (
            running is None or last.get("running") == running
        ):
            return last
        time.sleep(0.02)
    raise SmokeFailure(f"timed out waiting for {marker!r}: {last!r}")


def toolchain_version(language: str) -> str:
    commands = {
        "rust": ["rustc", "--version"],
        "go": ["go", "version"],
        "cpp": ["c++", "--version"],
    }
    try:
        result = subprocess.run(
            commands[language],
            check=True,
            capture_output=True,
            text=True,
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
    if result["case_id"] != "T08":
        raise SmokeFailure("result case_id must be T08")
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
    session_id = f"t08-{uuid.uuid4().hex}"
    args.results_dir.mkdir(parents=True, exist_ok=True)
    raw_dir = args.results_dir / f"T08-attach-{session_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    repo_root = args.binary.resolve().parents[3]

    def display_path(path: Path) -> str:
        return str(path.resolve().relative_to(repo_root))

    output_log = raw_dir / "session.pty.log"
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
    checks: dict[str, bool] = {}

    with tempfile.TemporaryDirectory(prefix="qwe-attach.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"
        database = raw_dir / "tasks.sqlite"
        daemon_log = raw_dir / "daemon.log"
        environment = os.environ.copy()
        environment["QWE_LEASE_MS"] = "60000"
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
                race_session = f"t08-race-{uuid.uuid4().hex}"
                race_responses: list[dict[str, Any]] = []

                def race_start(index: int) -> None:
                    race_responses.append(
                        request(
                            socket_path,
                            "SESSION_START",
                            race_session,
                            str(raw_dir / f"race-{index}.pty.log"),
                            "sh",
                            "-c",
                            "sleep 0.6",
                        )
                    )

                threads = [
                    threading.Thread(target=race_start, args=(index,))
                    for index in (1, 2)
                ]
                for thread in threads:
                    thread.start()
                for thread in threads:
                    thread.join()
                successful_starts = sum(
                    response.get("ok") is True for response in race_responses
                )
                duplicate_errors = sum(
                    response.get("error") == "already_exists"
                    for response in race_responses
                )
                checks["duplicate_start_rejected"] = (
                    successful_starts == 1 and duplicate_errors == 1
                )
                time.sleep(0.7)

                success(
                    request(socket_path, "LEASE_CLAIM", session_id, "client-a"),
                    "LEASE_CLAIM",
                )
                started_response = success(
                    request(
                        socket_path,
                        "SESSION_START",
                        session_id,
                        str(output_log),
                        "sh",
                    ),
                    "SESSION_START",
                )
                checks["session_started"] = started_response.get("running") is True

                wrong_owner = request(
                    socket_path, "SESSION_ATTACH", session_id, "client-b"
                )
                checks["wrong_owner_rejected"] = (
                    wrong_owner.get("ok") is False
                    and wrong_owner.get("error") == "not_owner"
                )

                attached = success(
                    request(socket_path, "SESSION_ATTACH", session_id, "client-a"),
                    "SESSION_ATTACH",
                )
                checks["initial_attach"] = attached.get("attached") is True
                malformed = request(
                    socket_path,
                    "SESSION_INPUT",
                    session_id,
                    "client-a",
                    "xyz",
                )
                checks["malformed_input_rejected"] = (
                    malformed.get("ok") is False
                    and malformed.get("error") == "invalid_request"
                )

                initial_input = (
                    b"printf 'QWE_INITIAL\\n'; "
                    b"(sleep 0.25; printf 'QWE_DETACHED\\n') &\r"
                ).hex().encode()
                success(
                    request(
                        socket_path,
                        "SESSION_INPUT",
                        session_id,
                        "client-a",
                        initial_input.decode(),
                    ),
                    "SESSION_INPUT",
                )
                wait_for_text(socket_path, session_id, b"QWE_INITIAL")
                checks["initial_input"] = True

                detached = success(
                    request(socket_path, "SESSION_DETACH", session_id, "client-a"),
                    "SESSION_DETACH",
                )
                checks["detach"] = detached.get("attached") is False
                time.sleep(0.35)

                detached_status = wait_for_text(
                    socket_path, session_id, b"QWE_DETACHED", running=True
                )
                checks["pty_survives"] = detached_status.get("attached") is False
                lease = success(
                    request(socket_path, "LEASE_STATUS", session_id),
                    "LEASE_STATUS",
                )
                checks["lease_preserved"] = lease.get("owner") == "client-a"

                rejected = request(
                    socket_path,
                    "SESSION_INPUT",
                    session_id,
                    "client-a",
                    b"printf 'QWE_BAD\\n'\r".hex(),
                )
                checks["detached_input_rejected"] = (
                    rejected.get("ok") is False
                    and rejected.get("error") == "not_attached"
                )

                reattached = success(
                    request(socket_path, "SESSION_ATTACH", session_id, "client-a"),
                    "SESSION_ATTACH",
                )
                replay = snapshot_text(reattached)
                checks["reattach"] = reattached.get("attached") is True
                checks["screen_restored"] = (
                    b"QWE_INITIAL" in replay and b"QWE_DETACHED" in replay
                )
                checks["detached_input_not_executed"] = b"QWE_BAD" not in replay

                success(
                    request(
                        socket_path,
                        "SESSION_INPUT",
                        session_id,
                        "client-a",
                        b"printf 'QWE_REATTACHED\\n'; exit\r".hex(),
                    ),
                    "SESSION_INPUT",
                )
                final = wait_for_text(
                    socket_path, session_id, b"QWE_REATTACHED", running=False
                )
                checks["input_after_reattach"] = True
                checks["process_exited_cleanly"] = final.get("running") is False
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)

    passed = all(checks.values())
    result = {
        "case_id": "T08",
        "language": args.language,
        "status": "blocked" if passed else "fail",
        "source_commit": source_commit,
        "environment": {
            "os": host_platform.system(),
            "architecture": host_platform.machine(),
            "toolchain": toolchain_version(args.language),
            "environment_record": display_path(environment_record),
        },
        "command": f"attach-smoke {args.language}",
        "started_at": started_iso,
        "duration_ms": round((time.monotonic() - started_monotonic) * 1000),
        "metrics": {"output_bytes": final.get("output_bytes", 0)},
        "checks": checks,
        "evidence": [
            display_path(output_log),
            display_path(daemon_log),
            display_path(environment_record),
        ],
        "known_limitations": [
            "Snapshot is the complete raw PTY stream, not a compact VT state delta.",
            "Interactive manual screen review remains required.",
        ],
    }
    run_stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(started_wall))
    validate_result_document(result)
    result_path = args.results_dir / (
        f"T08-{source_commit}-{run_stamp}-{os.getpid()}.json"
    )
    result_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "checks": checks}))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
