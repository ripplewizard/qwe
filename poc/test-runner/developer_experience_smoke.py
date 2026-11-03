#!/usr/bin/env python3
"""Measure DX01-DX03 and validate the minimal persisted TUI."""

from __future__ import annotations

import argparse
import json
import os
import platform as host_platform
import re
import shutil
import signal
import socket
import statistics
import subprocess
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any


ANSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")


class SmokeFailure(RuntimeError):
    pass


def request(socket_path: Path, command: str, *fields: str) -> dict[str, Any]:
    request_id = f"dx-{uuid.uuid4().hex}"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(5)
        client.connect(str(socket_path))
        client.sendall("\t".join((command, request_id, *fields)).encode() + b"\n")
        response = b""
        while not response.endswith(b"\n"):
            chunk = client.recv(4096)
            if not chunk:
                raise SmokeFailure(f"{command} closed without a response")
            response += chunk
    payload = json.loads(response)
    if payload.get("request_id") != request_id:
        raise SmokeFailure(f"{command} did not echo request ID")
    return payload


def wait_ready(socket_path: Path, process: subprocess.Popen[bytes]) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise SmokeFailure("daemon exited before becoming ready")
        if socket_path.exists():
            try:
                response = request(socket_path, "STATUS", "__dx_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready")


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * fraction))]


def timing_summary(values: list[float]) -> dict[str, float | int]:
    return {
        "samples": len(values),
        "median_ms": round(statistics.median(values), 3),
        "p95_ms": round(percentile(values, 0.95), 3),
        "max_ms": round(max(values), 3),
    }


def dependency_count(repo_root: Path, language: str) -> int:
    if language == "rust":
        lines = (repo_root / "poc/rust/Cargo.toml").read_text().splitlines()
        in_dependencies = False
        count = 0
        for line in lines:
            stripped = line.strip()
            if stripped == "[dependencies]":
                in_dependencies = True
                continue
            if in_dependencies and stripped.startswith("["):
                break
            if in_dependencies and "=" in stripped and not stripped.startswith("#"):
                count += 1
        return count
    if language == "go":
        text = (repo_root / "poc/go/go.mod").read_text()
        return len(
            re.findall(
                r"^\s*(?:require\s+)?[A-Za-z0-9._/-]+\s+v[^\s]+$",
                text,
                re.MULTILINE,
            )
        )
    text = (repo_root / "poc/cpp/CMakeLists.txt").read_text()
    return len(re.findall(r"find_package\(|pkg_check_modules\(", text))


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


def source_commit(repo_root: Path) -> str:
    commit = subprocess.run(
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
        commit += "-dirty"
    return commit


def write_result(
    *,
    case_id: str,
    status: str,
    language: str,
    commit: str,
    repo_root: Path,
    results_dir: Path,
    started_iso: str,
    started_monotonic: float,
    command: str,
    metrics: dict[str, Any],
    checks: dict[str, bool],
    evidence: list[Path],
    environment_record: Path,
    limitations: list[str],
) -> None:
    result = {
        "case_id": case_id,
        "language": language,
        "status": status,
        "source_commit": commit,
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
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    path = results_dir / f"{case_id}-{commit}-{stamp}-{os.getpid()}.json"
    path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


def dx01(
    binary: Path,
    repo_root: Path,
    work_dir: Path,
) -> tuple[dict[str, bool], dict[str, Any], list[Path]]:
    cli_latencies: list[float] = []
    for _ in range(12):
        started = time.monotonic()
        run = subprocess.run(
            [str(binary), "probe"],
            capture_output=True,
            text=True,
            check=False,
        )
        cli_latencies.append((time.monotonic() - started) * 1000)
        if run.returncode != 0 or '"contract":"qwe-poc/0.1"' not in run.stdout:
            raise SmokeFailure("probe command failed")

    invalid = subprocess.run(
        [str(binary), "definitely-not-a-command"],
        capture_output=True,
        text=True,
        check=False,
    )
    error_log = work_dir / "DX01.error.txt"
    error_log.write_text(
        f"exit={invalid.returncode}\nstdout={invalid.stdout}\nstderr={invalid.stderr}\n",
        encoding="utf-8",
    )

    socket_path = Path(f"/tmp/qwe-dx01-{uuid.uuid4().hex}.sock")
    database = work_dir / "dx01.sqlite"
    daemon_log = work_dir / "DX01.daemon.log"
    ipc_latencies: list[float] = []
    with daemon_log.open("wb") as log:
        daemon = subprocess.Popen(
            [str(binary), "daemon", str(socket_path), str(database)],
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            wait_ready(socket_path, daemon)
            for _ in range(30):
                started = time.monotonic()
                response = request(socket_path, "STATUS", "missing-dx01")
                ipc_latencies.append((time.monotonic() - started) * 1000)
                if response.get("error") != "not_found":
                    raise SmokeFailure("STATUS error contract mismatch")
        finally:
            daemon.terminate()
            daemon.wait(timeout=2)
            if socket_path.exists():
                socket_path.unlink()

    checks = {
        "probe_contract": len(cli_latencies) == 12,
        "invalid_command_clear": (
            invalid.returncode == 2
            and "unsupported command" in invalid.stderr.lower()
        ),
        "ipc_error_clear": len(ipc_latencies) == 30,
        "startup_latency_recorded": True,
        "dependency_count_recorded": dependency_count(repo_root, binary.parent.name) > 0,
    }
    cli_summary = timing_summary(cli_latencies)
    ipc_summary = timing_summary(ipc_latencies)
    metrics = {
        "cli_startup_samples": cli_summary["samples"],
        "cli_startup_median_ms": cli_summary["median_ms"],
        "cli_startup_p95_ms": cli_summary["p95_ms"],
        "cli_startup_max_ms": cli_summary["max_ms"],
        "ipc_missing_status_samples": ipc_summary["samples"],
        "ipc_missing_status_median_ms": ipc_summary["median_ms"],
        "ipc_missing_status_p95_ms": ipc_summary["p95_ms"],
        "ipc_missing_status_max_ms": ipc_summary["max_ms"],
        "declared_dependencies": dependency_count(repo_root, binary.parent.name),
        "time_to_demo_seconds_value": None,
        "time_to_demo_seconds_reason": (
            "The POC was built incrementally; retrospective wall-clock "
            "development time is not reconstructable."
        ),
    }
    return checks, metrics, [error_log, daemon_log]


def dashboard_text(binary: Path, database: Path, work_dir: Path) -> tuple[bytes, Path]:
    screen = subprocess.run(
        [str(binary), "dashboard", str(database)],
        check=False,
        capture_output=True,
        env={**os.environ, "TERM": "xterm-256color"},
    )
    raw = screen.stdout + screen.stderr
    log = work_dir / "DX02.dashboard.log"
    log.write_bytes(raw)
    if screen.returncode != 0:
        raise SmokeFailure(
            f"dashboard exited {screen.returncode}: {raw.decode(errors='replace')}"
        )
    return ANSI.sub(b"", raw), log


def dx02(binary: Path, work_dir: Path) -> tuple[dict[str, bool], dict[str, Any], list[Path]]:
    socket_path = Path(f"/tmp/qwe-dx02-{uuid.uuid4().hex}.sock")
    database = work_dir / "dx02.sqlite"
    daemon_log = work_dir / "DX02.daemon.log"
    task_id = ""
    pid = 0
    with daemon_log.open("wb") as log:
        daemon = subprocess.Popen(
            [str(binary), "daemon", str(socket_path), str(database)],
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            wait_ready(socket_path, daemon)
            started = request(
                socket_path,
                "START",
                "dx02-dashboard-task",
                str(work_dir / "DX02.task.log"),
                "sh",
                "-c",
                "sleep 20",
            )
            if started.get("ok") is not True:
                raise SmokeFailure(f"DX02 task start failed: {started!r}")
            task_id = str(started["task_id"])
            pid = int(started["pid"])
            live_text, live_log = dashboard_text(binary, database, work_dir)
        finally:
            daemon.terminate()
            daemon.wait(timeout=2)
            if socket_path.exists():
                socket_path.unlink()
    persisted_text, persisted_log = dashboard_text(binary, database, work_dir)
    if pid:
        try:
            os.killpg(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass

    checks = {
        "alternate_screen": b"\x1b[?1049h" in live_log.read_bytes(),
        "dashboard_title": b"QWE POC DASHBOARD" in live_text,
        "live_task_visible": b"dx02-dashboard-task" in live_text,
        "sqlite_after_daemon_exit": b"dx02-dashboard-task" in persisted_text,
        "task_state_visible": b"state=stopped" in persisted_text
        or b"state=running" in persisted_text,
        "task_id_visible": task_id.encode() in persisted_text,
    }
    metrics = {
        "database_bytes": database.stat().st_size,
        "live_screen_bytes": len(live_text),
        "persisted_screen_bytes": len(persisted_text),
    }
    return checks, metrics, [live_log, persisted_log, daemon_log]


def run_timed(
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    log: Path,
) -> tuple[float, subprocess.CompletedProcess[str]]:
    started = time.monotonic()
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    duration = time.monotonic() - started
    log.write_text(
        f"$ {' '.join(command)}\nexit={result.returncode}\n"
        f"--- stdout ---\n{result.stdout}\n--- stderr ---\n{result.stderr}\n",
        encoding="utf-8",
    )
    return duration, result


def dx03(
    binary: Path,
    repo_root: Path,
    work_dir: Path,
) -> tuple[dict[str, bool], dict[str, Any], list[Path]]:
    language = binary.parent.name
    build_dir = work_dir / f"DX03-{language}-build"
    build_dir.mkdir()
    env = os.environ.copy()
    build_log = work_dir / "DX03.build.log"
    failure_log = work_dir / "DX03.compiler-error.log"
    if language == "rust":
        env["CARGO_TARGET_DIR"] = str(build_dir / "target")
        build_command = [
            "cargo",
            "build",
            "--release",
            "--manifest-path",
            str(repo_root / "poc/rust/Cargo.toml"),
        ]
        broken = build_dir / "broken.rs"
        broken.write_text("fn main() { let value: i32 = \"no\"; }\n", encoding="utf-8")
        failure_command = ["rustc", str(broken)]
    elif language == "go":
        env["GOCACHE"] = str(build_dir / "cache")
        env["GOPATH"] = str(build_dir / "gopath")
        build_command = [
            "go",
            "build",
            "-trimpath",
            "-o",
            str(build_dir / "qwe-poc"),
            "./cmd/qwe-poc",
        ]
        broken = build_dir / "broken.go"
        broken.write_text("package main\nfunc main() { var value int = \"no\" }\n")
        failure_command = ["go", "run", str(broken)]
    elif language == "cpp":
        configure = [
            "cmake",
            "-S",
            str(repo_root / "poc/cpp"),
            "-B",
            str(build_dir),
            "-G",
            "Ninja",
            "-DCMAKE_BUILD_TYPE=Release",
        ]
        _, configure_result = run_timed(
            configure,
            cwd=repo_root,
            env=env,
            log=work_dir / "DX03.configure.log",
        )
        if configure_result.returncode != 0:
            raise SmokeFailure("C++ configure failed")
        build_command = ["cmake", "--build", str(build_dir)]
        broken = build_dir / "broken.cpp"
        broken.write_text(
            "#include <string>\nint main() { int value = \"no\"; }\n",
            encoding="utf-8",
        )
        failure_command = [
            "c++",
            "-std=c++20",
            "-fsyntax-only",
            str(broken),
        ]
    else:
        raise SmokeFailure(f"unsupported DX language: {language}")

    duration, build = run_timed(
        build_command,
        cwd=repo_root / "poc/go" if language == "go" else repo_root,
        env=env,
        log=build_log,
    )
    failure_duration, failure = run_timed(
        failure_command,
        cwd=work_dir,
        env=env,
        log=failure_log,
    )
    diagnostic = (failure.stdout + failure.stderr).lower()
    checks = {
        "clean_build_succeeded": build.returncode == 0,
        "build_duration_recorded": duration > 0,
        "compiler_failure_rejected": failure.returncode != 0,
        "compiler_failure_has_location": "broken." in diagnostic,
        "compiler_failure_explains_type": any(
            word in diagnostic for word in ("type", "mismatch", "cannot", "invalid")
        ),
    }
    metrics = {
        "clean_build_ms": round(duration * 1000, 3),
        "compiler_failure_ms": round(failure_duration * 1000, 3),
        "build_output_bytes": len(build.stdout) + len(build.stderr),
        "compiler_error_bytes": len(failure.stdout) + len(failure.stderr),
    }
    return checks, metrics, [build_log, failure_log]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--language", required=True, choices=("rust", "go", "cpp"))
    parser.add_argument("--platform", required=True)
    parser.add_argument("--results-dir", required=True, type=Path)
    args = parser.parse_args()
    repo_root = args.binary.resolve().parents[3]
    args.results_dir.mkdir(parents=True, exist_ok=True)
    work_dir = args.results_dir / f"developer-experience-{uuid.uuid4().hex}"
    work_dir.mkdir()
    environment_record = work_dir / "environment.txt"
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
    commit = source_commit(repo_root)
    started = time.monotonic()
    started_iso = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    failures: list[str] = []
    cases = [
        ("DX01", *dx01(args.binary, repo_root, work_dir)),
        ("DX02", *dx02(args.binary, work_dir)),
        ("DX03", *dx03(args.binary, repo_root, work_dir)),
    ]
    for case_id, checks, metrics, evidence in cases:
        passed = all(checks.values())
        if not passed:
            failures.append(case_id)
        write_result(
            case_id=case_id,
            status="pass" if passed else "fail",
            language=args.language,
            commit=commit,
            repo_root=repo_root,
            results_dir=args.results_dir,
            started_iso=started_iso,
            started_monotonic=started,
            command=f"developer-experience-smoke {args.language} {case_id}",
            metrics=metrics,
            checks=checks,
            evidence=evidence,
            environment_record=environment_record,
            limitations=[
                "POC measurements are single-host comparisons, not production benchmarks.",
                "Retrospective development time is not reconstructable from source history.",
            ],
        )
        print(json.dumps({"case_id": case_id, "status": "pass" if passed else "fail", "checks": checks}))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
