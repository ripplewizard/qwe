#!/usr/bin/env python3
"""Exercise the T09 session-lease protocol through the daemon socket."""

from __future__ import annotations

import argparse
import datetime
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
    request_id = f"lease-test-{uuid.uuid4().hex}"
    line = "\t".join((command, request_id, *fields))
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(2)
        client.connect(str(socket_path))
        client.sendall(f"{line}\n".encode())
        response = b""
        while not response.endswith(b"\n"):
            chunk = client.recv(4096)
            if not chunk:
                raise SmokeFailure(f"{command} closed without a complete response")
            response += chunk
    try:
        payload = json.loads(response)
    except json.JSONDecodeError as error:
        raise SmokeFailure(f"{command} returned invalid JSON: {response!r}") from error
    if payload.get("request_id") != request_id:
        raise SmokeFailure(
            f"{command} did not echo request ID {request_id}: {payload!r}"
        )
    if payload.get("ok") is True and command.startswith("LEASE_"):
        required = {
            "session_id",
            "state",
            "owner",
            "remaining_ms",
            "output_bytes",
        }
        missing = required.difference(payload)
        if missing:
            raise SmokeFailure(
                f"{command} omitted required lease fields "
                f"{sorted(missing)}: {payload!r}"
            )
    return payload


def wait_ready(socket_path: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if process.poll() is not None:
            stdout, _ = process.communicate()
            raise SmokeFailure(f"daemon exited before becoming ready:\n{stdout}")
        if socket_path.exists():
            try:
                response = request(socket_path, "STATUS", "__lease_ready__")
                if response.get("error") == "not_found":
                    return
            except (OSError, SmokeFailure):
                pass
        time.sleep(0.02)
    raise SmokeFailure("daemon did not become ready within 5 seconds")


def expect_success(response: dict[str, Any], operation: str) -> dict[str, Any]:
    if response.get("ok") is not True:
        raise SmokeFailure(f"{operation} failed: {response!r}")
    return response


def expect_error(
    response: dict[str, Any], operation: str, error_code: str
) -> dict[str, Any]:
    if response.get("ok") is not False or response.get("error") != error_code:
        raise SmokeFailure(
            f"{operation} expected error {error_code!r}: {response!r}"
        )
    return response


def heartbeat(
    socket_path: Path,
    session_id: str,
    client_id: str,
    duration_ms: int,
    transitions: list[dict[str, Any]],
    scenario_started: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + duration_ms / 1000
    first_response: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        response = expect_success(
            request(socket_path, "LEASE_TOUCH", session_id, client_id),
            "LEASE_TOUCH heartbeat",
        )
        transitions.append(
            {
                "action": "HEARTBEAT",
                "at_utc": datetime.datetime.now(
                    datetime.timezone.utc
                ).isoformat(),
                "elapsed_ms": round((time.monotonic() - scenario_started) * 1000),
                "response": response,
            }
        )
        first_response = first_response or response
        time.sleep(0.05)
    return first_response or expect_success(
        request(socket_path, "LEASE_TOUCH", session_id, client_id),
        "LEASE_TOUCH heartbeat",
    )


def run_scenario(
    socket_path: Path,
    scenario: dict[str, Any],
    last_error: str | None,
    raw_dir: Path,
    transitions: list[dict[str, Any]],
) -> tuple[dict[str, Any], str | None]:
    session_id = f"{scenario['id']}-{uuid.uuid4().hex}"
    last_response: dict[str, Any] | None = None
    scenario_started = time.monotonic()

    def record(
        action: str,
        arguments: list[Any],
        response: dict[str, Any] | None = None,
    ) -> None:
        transition: dict[str, Any] = {
            "action": action,
            "at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "elapsed_ms": round((time.monotonic() - scenario_started) * 1000),
        }
        if arguments:
            transition["arguments"] = arguments
        if response is not None:
            transition["response"] = response
        transitions.append(transition)

    for step in scenario["steps"]:
        action = step[0]
        arguments = step[1:]
        match action:
            case "CLAIM":
                response = request(
                    socket_path, "LEASE_CLAIM", session_id, arguments[0]
                )
                record("CLAIM", arguments, response)
                if len(arguments) > 1:
                    expect_error(response, "LEASE_CLAIM", arguments[1])
                    last_error = response["error"]
                else:
                    last_response = expect_success(response, "LEASE_CLAIM")
            case "TOUCH":
                response = request(
                    socket_path, "LEASE_TOUCH", session_id, arguments[0]
                )
                record("TOUCH", arguments, response)
                if response.get("ok") is True:
                    last_response = response
                else:
                    expect_error(response, "LEASE_TOUCH", "not_owner")
                    last_error = response["error"]
            case "RELEASE":
                response = request(
                    socket_path, "LEASE_RELEASE", session_id, arguments[0]
                )
                record("RELEASE", arguments, response)
                if len(arguments) > 1:
                    expect_error(response, "LEASE_RELEASE", arguments[1])
                    last_error = response["error"]
                else:
                    last_response = expect_success(response, "LEASE_RELEASE")
            case "STATUS":
                response = request(socket_path, "LEASE_STATUS", session_id)
                record("STATUS", arguments, response)
                last_response = expect_success(response, "LEASE_STATUS")
            case "TAKEOVER":
                response = request(
                    socket_path, "LEASE_TAKEOVER", session_id, arguments[0]
                )
                record("TAKEOVER", arguments, response)
                if len(arguments) > 1:
                    expect_error(response, "LEASE_TAKEOVER", arguments[1])
                    last_error = response["error"]
                else:
                    last_response = expect_success(response, "LEASE_TAKEOVER")
            case "WAIT":
                record("WAIT", arguments)
                time.sleep(arguments[0] / 1000)
            case "HEARTBEAT":
                last_response = heartbeat(
                    socket_path,
                    session_id,
                    arguments[0],
                    arguments[1],
                    transitions,
                    scenario_started,
                )
            case "EMIT_OUTPUT":
                raw_dir.mkdir(parents=True, exist_ok=True)
                output_log = raw_dir / f"{scenario['id']}.pty.log"
                response = expect_success(
                    request(
                        socket_path,
                        "SESSION_START",
                        session_id,
                        str(output_log),
                        "sh",
                        "-c",
                        "sleep 0.05; "
                        "i=0; while [ $i -lt 15 ]; do "
                        "printf QWE_LEASE_OUTPUT; sleep 0.04; "
                        "i=$((i+1)); done; sleep 0.6",
                    ),
                    "SESSION_START",
                )
                record("SESSION_START", arguments, response)
                deadline = time.monotonic() + 0.6
                observed_output = False
                while time.monotonic() < deadline:
                    response = request(socket_path, "LEASE_STATUS", session_id)
                    record("STATUS", [], response)
                    if response.get("output_bytes", 0) >= 1:
                        observed_output = True
                        break
                    time.sleep(0.02)
                if not observed_output:
                    raise SmokeFailure("PTY output was not observed by the daemon")
                if b"QWE_LEASE_OUTPUT" not in output_log.read_bytes():
                    raise SmokeFailure(
                        "PTY output log did not contain the expected marker"
                    )
                time.sleep(arguments[0] / 1000)
            case _:
                raise SmokeFailure(f"unsupported scenario action: {action}")
    if last_response is None:
        raise SmokeFailure(f"scenario {scenario['id']} produced no response")
    return last_response, last_error


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
    allowed = required
    if set(result) != allowed:
        raise SmokeFailure(
            f"result fields differ from schema: {sorted(set(result) ^ allowed)}"
        )
    if result["case_id"] != "T09":
        raise SmokeFailure("result case_id must be T09")
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
    parser.add_argument("--scenarios", required=True, type=Path)
    parser.add_argument("--language", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--results-dir", required=True, type=Path)
    args = parser.parse_args()

    spec = json.loads(args.scenarios.read_text(encoding="utf-8"))
    scenario_results: list[dict[str, Any]] = []
    failures = 0
    started_at = time.time()
    started_iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
    run_stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(started_at))
    run_id = f"{run_stamp}-{os.getpid()}"
    raw_dir = args.results_dir / f"T09-lease-{run_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    repo_root = args.binary.resolve().parents[3]

    def display_path(path: Path) -> str:
        return str(path.resolve().relative_to(repo_root))

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
                f"lease_ms={spec['lease_ms']}",
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
        ["git", "-C", str(repo_root), "status", "--porcelain"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout:
        source_commit += "-dirty"

    with tempfile.TemporaryDirectory(prefix="qwe-lease-smoke.") as temp_name:
        socket_path = Path(temp_name) / "daemon.sock"
        database_path = raw_dir / "lease.sqlite"
        log_path = raw_dir / "daemon.log"
        environment = os.environ.copy()
        environment["QWE_LEASE_MS"] = str(spec["lease_ms"])
        with log_path.open("w", encoding="utf-8") as daemon_log:
            process = subprocess.Popen(
                [str(args.binary), "daemon", str(socket_path), str(database_path)],
                stdout=daemon_log,
                stderr=subprocess.STDOUT,
                env=environment,
                text=True,
            )
            try:
                wait_ready(socket_path, process)
                for scenario in spec["scenarios"]:
                    last_error: str | None = None
                    transitions: list[dict[str, Any]] = []
                    try:
                        response, last_error = run_scenario(
                            socket_path,
                            scenario,
                            last_error,
                            raw_dir,
                            transitions,
                        )
                        expected = scenario["expect"]
                        for field, value in expected.items():
                            if field == "min_output_bytes":
                                if response.get("output_bytes", 0) < value:
                                    raise SmokeFailure(
                                        f"{scenario['id']} expected at least "
                                        f"{value} output bytes, observed "
                                        f"{response.get('output_bytes', 0)}"
                                    )
                                continue
                            observed = (
                                last_error if field == "last_error" else response.get(field)
                            )
                            if observed != value:
                                raise SmokeFailure(
                                    f"{scenario['id']} expected {field}={value!r}, "
                                    f"observed {observed!r}"
                                )
                        scenario_results.append(
                            {
                                "id": scenario["id"],
                                "status": "pass",
                                "response": response,
                                "last_error": last_error,
                                "transitions": transitions,
                            }
                        )
                    except (OSError, SmokeFailure) as error:
                        failures += 1
                        scenario_results.append(
                            {
                                "id": scenario["id"],
                                "status": "fail",
                                "reason": str(error),
                                "transitions": transitions,
                            }
                        )
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)

        scenario_log = raw_dir / "scenarios.json"
        scenario_log.write_text(
            json.dumps(scenario_results, indent=2) + "\n",
            encoding="utf-8",
        )
        checks = {
            result["id"].replace("-", "_"): result["status"] == "pass"
            for result in scenario_results
        }
        output_result = next(
            (
                result
                for result in scenario_results
                if result["id"] == "output-is-not-input"
            ),
            {},
        )
        output_response = output_result.get("response", {})
        result_path = args.results_dir / f"T09-{source_commit}-{run_id}.json"
        duration_ms = round((time.time() - started_at) * 1000)
        result_document = {
            "case_id": "T09",
            "language": args.language,
            "status": "fail" if failures else "pass",
            "source_commit": source_commit,
            "environment": {
                "os": host_platform.system(),
                "architecture": host_platform.machine(),
                "toolchain": toolchain_version(args.language),
                "environment_record": display_path(environment_record),
            },
            "command": (
                f"just lease-smoke {args.language} "
                f"(QWE_LEASE_MS={spec['lease_ms']})"
            ),
            "started_at": started_iso,
            "duration_ms": duration_ms,
            "metrics": {
                "lease_ms": spec["lease_ms"],
                "output_bytes": output_response.get("output_bytes", 0),
            },
            "checks": checks,
            "evidence": [
                display_path(scenario_log),
                display_path(log_path),
                display_path(environment_record),
                *[
                    display_path(path)
                    for path in sorted(raw_dir.glob("*.pty.log"))
                ],
            ],
            "known_limitations": [
                "Lease state is in-memory for this POC slice.",
                "Attach/detach and interactive input remain T08 scope.",
            ],
        }
        validate_result_document(result_document)
        result_path.write_text(
            json.dumps(result_document, indent=2) + "\n",
            encoding="utf-8",
        )
        final_status = "fail" if failures else "pass"
        print(json.dumps({"status": final_status,
                          "result": str(result_path), "failures": failures}))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
