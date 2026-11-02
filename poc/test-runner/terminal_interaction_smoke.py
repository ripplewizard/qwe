#!/usr/bin/env python3
"""Exercise automated T04, T11, and T12 terminal preflights."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import signal
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
    request_id = f"terminal-interaction-{uuid.uuid4().hex}"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(3)
        client.connect(str(socket_path))
        client.sendall("\t".join((command, request_id, *fields)).encode() + b"\n")
        response = b""
        while not response.endswith(b"\n"):
            chunk = client.recv(65536)
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
                response = request(socket_path, "STATUS", "__terminal_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready")


def snapshot(response: dict[str, Any]) -> bytes:
    try:
        return bytes.fromhex(response["snapshot_hex"])
    except (KeyError, ValueError) as error:
        raise SmokeFailure(f"invalid snapshot response: {response!r}") from error


def status(socket_path: Path, session_id: str) -> dict[str, Any]:
    return success(
        request(socket_path, "SESSION_STATUS", session_id),
        "SESSION_STATUS",
    )


def wait_for(
    socket_path: Path,
    session_id: str,
    predicate,
    description: str,
    timeout: float = 4,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        last = status(socket_path, session_id)
        if predicate(last, snapshot(last)):
            return last
        time.sleep(0.025)
    raise SmokeFailure(f"timed out waiting for {description}: {last!r}")


def input_bytes(
    socket_path: Path,
    session_id: str,
    client_id: str,
    data: bytes,
) -> dict[str, Any]:
    return success(
        request(
            socket_path,
            "SESSION_INPUT",
            session_id,
            client_id,
            data.hex(),
        ),
        "SESSION_INPUT",
    )


def start_session(
    socket_path: Path,
    session_id: str,
    client_id: str,
    log_path: Path,
    program: str,
    *arguments: str,
) -> dict[str, Any]:
    success(
        request(socket_path, "LEASE_CLAIM", session_id, client_id),
        "LEASE_CLAIM",
    )
    started = success(
        request(
            socket_path,
            "SESSION_START",
            session_id,
            str(log_path),
            program,
            *arguments,
        ),
        "SESSION_START",
    )
    success(
        request(socket_path, "SESSION_ATTACH", session_id, client_id),
        "SESSION_ATTACH",
    )
    return started


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


def t04(
    binary: Path,
    socket_path: Path,
    raw_dir: Path,
) -> tuple[dict[str, bool], dict[str, Any], bytes]:
    session_id = f"t04-{uuid.uuid4().hex}"
    log_path = raw_dir / "T04.pty.log"
    start_session(socket_path, session_id, "client", log_path, "sh")
    input_bytes(
        socket_path,
        session_id,
        "client",
        b"printf 'QWE_HTop_START\\n'; htop\r",
    )
    first = wait_for(
        socket_path,
        session_id,
        lambda _, data: b"QWE_HTop_START" in data and b"Load average:" in data,
        "htop first frame",
    )
    first_bytes = first.get("output_bytes", 0)
    second = wait_for(
        socket_path,
        session_id,
        lambda response, _: response.get("output_bytes", 0) > first_bytes,
        "htop refresh output",
        timeout=4,
    )
    second_bytes = second.get("output_bytes", 0)
    first_snapshot = snapshot(first)
    second_snapshot = snapshot(second)
    checks = {
        "full_screen_started": b"\x1b[?1049h" in second_snapshot,
        "first_frame_rendered": b"Tasks:" in first_snapshot,
        "dynamic_refresh_observed": second_bytes > first_bytes,
        "frame_changed": second_snapshot != first_snapshot,
    }
    input_bytes(socket_path, session_id, "client", b"q")
    input_bytes(
        socket_path,
        session_id,
        "client",
        b"printf 'QWE_HTop__END\\n'; exit\r",
    )
    final = wait_for(
        socket_path,
        session_id,
        lambda response, data: (
            b"QWE_HTop__END" in data and response.get("running") is False
        ),
        "htop clean exit",
    )
    checks["clean_exit"] = b"QWE_HTop__END" in snapshot(final)
    metrics = {
        "first_output_bytes": first_bytes,
        "second_output_bytes": second_bytes,
        "delta_bytes": second_bytes - first_bytes,
    }
    return checks, metrics, snapshot(first)


def t11(
    binary: Path,
    socket_path: Path,
    raw_dir: Path,
) -> tuple[dict[str, bool], dict[str, Any], bytes]:
    session_id = f"t11-{uuid.uuid4().hex}"
    log_path = raw_dir / "T11.pty.log"
    start_session(
        socket_path,
        session_id,
        "client",
        log_path,
        "/bin/bash",
        "--noprofile",
        "--norc",
        "-i",
    )
    input_bytes(
        socket_path,
        session_id,
        "client",
        b"PS1='QWE> '; printf 'QWE_JOB_READY\\n'\r",
    )
    wait_for(
        socket_path,
        session_id,
        lambda _, data: b"QWE_JOB_READY" in data,
        "job-control shell prompt",
    )

    input_bytes(socket_path, session_id, "client", b"sleep 30\r")
    time.sleep(0.25)
    input_bytes(socket_path, session_id, "client", b"\x03")
    input_bytes(
        socket_path,
        session_id,
        "client",
        b"printf 'QWE_AFTER_INT:%d\\n' \"$?\"\r",
    )
    after_int = wait_for(
        socket_path,
        session_id,
        lambda _, data: b"QWE_AFTER_INT:130" in data,
        "Ctrl-C status 130",
    )

    input_bytes(socket_path, session_id, "client", b"sleep 30\r")
    time.sleep(0.25)
    input_bytes(socket_path, session_id, "client", b"\x1a")
    input_bytes(socket_path, session_id, "client", b"jobs\r")
    stopped = wait_for(
        socket_path,
        session_id,
        lambda _, data: b"QWE_AFTER_INT:130" in data and b"Stopped" in data,
        "Ctrl-Z stopped job",
    )

    input_bytes(socket_path, session_id, "client", b"bg %1\r")
    input_bytes(socket_path, session_id, "client", b"jobs\r")
    background = wait_for(
        socket_path,
        session_id,
        lambda _, data: b"Running" in data and b"sleep 30" in data,
        "bg running job",
    )

    input_bytes(socket_path, session_id, "client", b"fg %1\r")
    time.sleep(0.25)
    input_bytes(socket_path, session_id, "client", b"\x03")
    input_bytes(
        socket_path,
        session_id,
        "client",
        b"printf 'QWE_AFTER_FG:%d\\n' \"$?\"; exit\r",
    )
    final = wait_for(
        socket_path,
        session_id,
        lambda response, data: (
            b"QWE_AFTER_FG:130" in data and response.get("running") is False
        ),
        "fg job interrupted and shell exit",
        timeout=5,
    )
    data = snapshot(final)
    checks = {
        "ctrl_c_status_130": b"QWE_AFTER_INT:130" in data,
        "ctrl_z_stopped": b"Stopped" in snapshot(stopped),
        "bg_running": b"Running" in snapshot(background),
        "fg_foreground_interrupt": b"QWE_AFTER_FG:130" in data,
        "shell_exited": final.get("running") is False,
    }
    metrics = {
        "int_output_bytes": after_int.get("output_bytes", 0),
        "final_output_bytes": final.get("output_bytes", 0),
    }
    return checks, metrics, data


def process_exists(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def t12(
    binary: Path,
    raw_dir: Path,
) -> tuple[dict[str, bool], dict[str, Any], bytes]:
    database = raw_dir / "T12.sqlite"
    daemon_log = raw_dir / "T12.daemon.log"
    session_id = f"t12-{uuid.uuid4().hex}"
    socket_directory = tempfile.TemporaryDirectory(prefix="qwe-t12.")
    socket_path = Path(socket_directory.name) / "daemon.sock"
    log_path = raw_dir / "T12.pty.log"
    environment = os.environ.copy()
    environment["QWE_LEASE_MS"] = "60000"
    environment["TERM"] = "xterm-256color"

    def launch_daemon() -> subprocess.Popen[str]:
        log = open(daemon_log, "a", encoding="utf-8")
        process = subprocess.Popen(
            [str(binary), "daemon", str(socket_path), str(database)],
            stdout=log,
            stderr=subprocess.STDOUT,
            env=environment,
            text=True,
        )
        setattr(process, "_qwe_log", log)
        wait_ready(socket_path, process)
        return process

    def stop_daemon(process: subprocess.Popen[str], sig: int) -> None:
        if process.poll() is None:
            process.send_signal(sig)
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
        log = getattr(process, "_qwe_log", None)
        if log is not None:
            log.close()

    daemon = launch_daemon()
    started = start_session(socket_path, session_id, "client", log_path, "sh")
    pid = int(started["pid"])
    before = input_bytes(
        socket_path,
        session_id,
        "client",
        (
            b"printf '\\033[?1049hQWE_T12_BEFORE\\n'; "
            b"printf 'QWE_T12_DETACHED\\n'\r"
        ),
    )
    before = wait_for(
        socket_path,
        session_id,
        lambda _, data: (
            b"QWE_T12_BEFORE" in data and b"QWE_T12_DETACHED" in data
        ),
        "initial terminal screen and detached output",
    )
    checks = {
        "screen_initialized": b"QWE_T12_BEFORE" in snapshot(before),
        "daemon_was_running": daemon.poll() is None,
    }
    stop_daemon(daemon, signal.SIGKILL)
    checks["daemon_restart_required"] = daemon.returncode == -signal.SIGKILL
    time.sleep(0.4)
    process_alive = process_exists(pid)
    checks["terminal_process_exit_observed"] = not process_alive

    restarted = launch_daemon()
    try:
        session_retry = request(socket_path, "SESSION_STATUS", session_id)
        session_recovered = session_retry.get("ok") is True
        checks["session_metadata_not_recovered"] = (
            session_retry.get("ok") is False
            and session_retry.get("error") == "not_found"
        )
        checks["pty_not_recovered"] = not session_recovered
        checks["screen_state_not_recovered"] = not session_recovered
        input_retry = request(
            socket_path,
            "SESSION_INPUT",
            session_id,
            "client",
            b"printf 'QWE_T12_AFTER\\n'\r".hex(),
        )
        checks["input_not_available"] = input_retry.get("ok") is False
    finally:
        stop_daemon(restarted, signal.SIGTERM)

    if process_alive:
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    checks["terminal_recovery_incomplete"] = not session_recovered
    metrics = {
        "terminal_pid": pid,
        "terminal_process_alive_after_daemon_restart": process_alive,
        "pre_restart_output_bytes": before.get("output_bytes", 0),
    }
    return checks, metrics, snapshot(before)


def result_document(
    *,
    case_id: str,
    language: str,
    status_value: str,
    source_commit: str,
    environment_record: Path,
    repo_root: Path,
    command: str,
    started_iso: str,
    started_monotonic: float,
    metrics: dict[str, Any],
    checks: dict[str, bool],
    evidence: list[Path],
    limitations: list[str],
) -> dict[str, Any]:
    return {
        "case_id": case_id,
        "language": language,
        "status": status_value,
        "source_commit": source_commit,
        "environment": {
            "os": host_platform.system(),
            "architecture": host_platform.machine(),
            "toolchain": toolchain_version(language),
            "environment_record": str(environment_record.relative_to(repo_root)),
        },
        "command": command,
        "started_at": started_iso,
        "duration_ms": round((time.monotonic() - started_monotonic) * 1000),
        "metrics": metrics,
        "checks": checks,
        "evidence": [str(path.relative_to(repo_root)) for path in evidence],
        "known_limitations": limitations,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--language", required=True, choices=("rust", "go", "cpp"))
    parser.add_argument("--platform", required=True)
    parser.add_argument("--results-dir", required=True, type=Path)
    args = parser.parse_args()

    repo_root = args.binary.resolve().parents[3]
    args.results_dir.mkdir(parents=True, exist_ok=True)
    raw_dir = args.results_dir / f"terminal-interaction-{uuid.uuid4().hex}"
    raw_dir.mkdir()
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
    dirty = subprocess.run(
        ["git", "-C", str(repo_root), "status", "--porcelain"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    if dirty:
        source_commit += "-dirty"

    failures: list[str] = []
    started_monotonic = time.monotonic()
    started_iso = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    with tempfile.TemporaryDirectory(prefix="qwe-terminal.") as temp_name:
        daemon_socket = Path(temp_name) / "daemon.sock"
        daemon_log = raw_dir / "T04-T11.daemon.log"
        database = raw_dir / "T04-T11.sqlite"
        environment = os.environ.copy()
        environment["QWE_LEASE_MS"] = "60000"
        environment["TERM"] = "xterm-256color"
        with daemon_log.open("w", encoding="utf-8") as log:
            daemon = subprocess.Popen(
                [str(args.binary), "daemon", str(daemon_socket), str(database)],
                stdout=log,
                stderr=subprocess.STDOUT,
                env=environment,
                text=True,
            )
            try:
                wait_ready(daemon_socket, daemon)
                t04_checks, t04_metrics, _ = t04(args.binary, daemon_socket, raw_dir)
                t11_checks, t11_metrics, _ = t11(args.binary, daemon_socket, raw_dir)
            finally:
                if daemon.poll() is None:
                    daemon.terminate()
                    try:
                        daemon.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        daemon.kill()
                        daemon.wait(timeout=2)

    t12_checks, t12_metrics, _ = t12(args.binary, raw_dir)
    if not all(t04_checks.values()):
        failures.append("T04")
    if not all(t11_checks.values()):
        failures.append("T11")
    if not t12_checks["screen_initialized"] or not t12_checks["daemon_was_running"]:
        failures.append("T12-setup")

    common_evidence = [
        environment_record,
        raw_dir / "T04.pty.log",
        raw_dir / "T11.pty.log",
        raw_dir / "T04-T11.daemon.log",
        raw_dir / "T12.pty.log",
        raw_dir / "T12.daemon.log",
    ]
    results = [
        result_document(
            case_id="T04",
            language=args.language,
            status_value="blocked" if all(t04_checks.values()) else "fail",
            source_commit=source_commit,
            environment_record=environment_record,
            repo_root=repo_root,
            command=f"{args.binary.name} daemon + SESSION_* htop preflight",
            started_iso=started_iso,
            started_monotonic=started_monotonic,
            metrics=t04_metrics,
            checks=t04_checks,
            evidence=common_evidence[:4],
            limitations=[
                "Automated launch, refresh, and exit checks only.",
                "Interactive htop visual behavior and resize remain manual.",
            ],
        ),
        result_document(
            case_id="T11",
            language=args.language,
            status_value="blocked" if all(t11_checks.values()) else "fail",
            source_commit=source_commit,
            environment_record=environment_record,
            repo_root=repo_root,
            command=f"{args.binary.name} daemon + SESSION_* Bash job-control preflight",
            started_iso=started_iso,
            started_monotonic=started_monotonic,
            metrics=t11_metrics,
            checks=t11_checks,
            evidence=[environment_record, raw_dir / "T11.pty.log", raw_dir / "T04-T11.daemon.log"],
            limitations=[
                "Automated key-sequence and exit-status checks only.",
                "Manual foreground/background terminal semantics review remains.",
            ],
        ),
        result_document(
            case_id="T12",
            language=args.language,
            status_value="blocked",
            source_commit=source_commit,
            environment_record=environment_record,
            repo_root=repo_root,
            command=f"{args.binary.name} daemon + SESSION_* daemon SIGKILL recovery preflight",
            started_iso=started_iso,
            started_monotonic=started_monotonic,
            metrics=t12_metrics,
            checks=t12_checks,
            evidence=[environment_record, raw_dir / "T12.pty.log", raw_dir / "T12.daemon.log"],
            limitations=[
                "This preflight records the current recovery boundary; it does not claim terminal recovery.",
                "PTY, terminal screen, session metadata, and interactive input are currently not recovered.",
                "Manual layer-by-layer review remains required.",
            ],
        ),
    ]

    run_stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    for result in results:
        path = args.results_dir / (
            f"{result['case_id']}-{source_commit}-{run_stamp}-{os.getpid()}.json"
        )
        path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(
            json.dumps(
                {
                    "case_id": result["case_id"],
                    "status": result["status"],
                    "checks": result["checks"],
                }
            )
        )

    if failures:
        print(json.dumps({"failures": failures}))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
