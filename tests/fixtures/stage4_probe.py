#!/usr/bin/env python3

import argparse
import json
import os
import signal
import socket
import sqlite3
import struct
import subprocess
import tempfile
import time
from pathlib import Path


PROTOCOL_VERSION = 1
MAX_FRAME_BYTES = 8 * 1024 * 1024
MAX_INPUT_BYTES = 1024 * 1024
SECRET = b"qwe-stage4-credential-value-7f3d"
SECRET_PREFIX = "qwe-stage4-credential-"


def receive_exact(stream, count):
    chunks = []
    remaining = count
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise EOFError("control socket closed during a frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def send_control(path, operation, payload, key=None, timeout=10):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"stage4-{time.time_ns()}",
        "idempotency_key": key,
        "operation": operation,
        "payload": payload,
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(timeout)
        stream.connect(path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        length = struct.unpack(">I", receive_exact(stream, 4))[0]
        assert length <= MAX_FRAME_BYTES
        response = json.loads(receive_exact(stream, length))
    result = response["result"]
    return result.get("Ok", result.get("Err"))


def request(path, operation, payload):
    return send_control(
        path,
        operation,
        payload,
        key=f"{operation.lower()}-{time.time_ns()}",
    )


def expect_error(path, operation, payload, code):
    result = request(path, operation, payload)
    assert result.get("code") == code, result
    return result


def wait_for_socket(path, process):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if process.poll() is not None:
            stderr = process.stderr.read().decode(errors="replace")
            raise RuntimeError(f"qwed exited with {process.returncode}: {stderr}")
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(path)
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"timed out waiting for {path}")


def start_qwed(binary, socket_path, credentials_dir):
    command = [
        binary,
        "--socket",
        socket_path,
        "--credentials-dir",
        credentials_dir,
    ]
    process = subprocess.Popen(
        command,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    wait_for_socket(socket_path, process)
    return process


def stop_qwed(process):
    if process is None or process.poll() is not None:
        return
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def create_session(
    path, session_id, program="sh", arguments=None, require_running=True, **extra
):
    payload = {
        "session_id": session_id,
        "program": program,
        "args": arguments or [],
    }
    payload.update(extra)
    created = request(path, "SESSION_CREATE", payload)
    if require_running:
        assert created.get("running") is True, created
    return created


def claim(path, session_id, owner="stage4-owner"):
    lease = request(
        path,
        "LEASE_CLAIM",
        {"session_id": session_id, "owner": owner, "ttl_ms": 60000},
    )
    assert lease["generation"] == 1, lease
    return lease["generation"]


def wait_session_snapshot(path, session_id, marker):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        snapshot = request(path, "SESSION_ATTACH", {"session_id": session_id})
        if marker in snapshot.get("screen", ""):
            return snapshot
        time.sleep(0.02)
    raise RuntimeError(f"snapshot for {session_id} did not contain {marker}")


def wait_session_exit(path, session_id):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        status = request(path, "SESSION_STATUS", {"session_id": session_id})
        if status.get("running") is False:
            return status
        time.sleep(0.02)
    raise RuntimeError(f"session {session_id} did not exit")


def assert_process_gone(pid):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.02)
    raise RuntimeError(f"process {pid} survived forced teardown")


def expect_closed_connection(path, prefix):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(path)
        stream.sendall(prefix)
        try:
            received = stream.recv(1)
        except (ConnectionResetError, BrokenPipeError):
            received = b""
    assert received == b"", "malformed control frame unexpectedly received a response"


def probe_credentials(path, process, credential_dir):
    status = request(path, "CONTROL_STATUS", {})
    assert status["credentials"]["backend"] == "filesystem", status
    assert status["credentials"]["names"] == ["STAGE4_KEY"], status
    assert status["capabilities"]["credentials"]["fd_inheritance"] is True, status

    insecure = credential_dir / "INSECURE"
    insecure.write_bytes(b"not-private")
    insecure.chmod(0o644)
    expect_error(
        path,
        "SESSION_CREATE",
        {
            "session_id": "insecure-credential",
            "program": "sh",
            "args": [],
            "credential": "INSECURE",
        },
        "credential_unavailable",
    )
    expect_error(
        path,
        "SESSION_CREATE",
        {
            "session_id": "missing-credential",
            "program": "sh",
            "args": [],
            "credential": "MISSING",
        },
        "credential_unavailable",
    )

    script = (
        "import os,time;"
        "fd=int(os.environ['QWE_CREDENTIAL_FD']);"
        "data=os.read(fd,16384);"
        "leak=sum(v.startswith('qwe-stage4-credential-') for v in os.environ.values());"
        "print(f'CREDENTIAL_BYTES={len(data)}',flush=True);"
        "print(f'CREDENTIAL_ENV_COUNT={leak}',flush=True);"
        "time.sleep(5)"
    )
    created = create_session(
        path,
        "credential-probe",
        program="python3",
        arguments=["-c", script],
        credential="STAGE4_KEY",
    )
    snapshot = wait_session_snapshot(
        path, "credential-probe", f"CREDENTIAL_BYTES={len(SECRET)}"
    )
    assert "CREDENTIAL_ENV_COUNT=0" in snapshot["screen"], snapshot
    assert SECRET.decode() not in snapshot["screen"], snapshot
    command_line = subprocess.check_output(
        ["ps", "-o", "command=", "-p", str(created["pid"])],
        text=True,
        stderr=subprocess.DEVNULL,
    )
    assert SECRET.decode() not in command_line, command_line
    return created


def probe_resources(path):
    capabilities = request(path, "CONTROL_STATUS", {})["capabilities"]["resources"]
    if capabilities["tasks_max"] == "unsupported":
        expect_error(
            path,
            "SESSION_CREATE",
            {
                "session_id": "process-limit",
                "program": "sh",
                "args": [],
                "resource_policy": {"tasks_max": 4},
            },
            "unsupported_capability",
        )
    else:
        process_script = (
            "import os,time; ok=0; err=0\n"
            "for _ in range(10):\n"
            " try:\n"
            "  pid=os.fork()\n"
            "  if pid==0: time.sleep(5); os._exit(0)\n"
            "  ok+=1\n"
            " except OSError: err+=1\n"
            " time.sleep(0.05)\n"
            "print(f'ok={ok} err={err}',flush=True)\n"
        )
        if capabilities["tasks_max"] == "hard_tasks":
            process_script += "raise SystemExit(7 if err else 0)"
        else:
            process_script += "time.sleep(10)"
        create_session(
            path,
            "process-limit",
            program="python3",
            arguments=["-c", process_script],
            resource_policy={"tasks_max": 5},
            require_running=False,
        )
        process_status = wait_session_exit(path, "process-limit")
        assert process_status.get("exit_code") != 0, process_status
        assert (
            process_status["resource_metrics"]["last_violation"] == "tasks_max"
        ), process_status

    if capabilities["memory_max"] == "unsupported":
        expect_error(
            path,
            "SESSION_CREATE",
            {
                "session_id": "memory-limit",
                "program": "sh",
                "args": [],
                "resource_policy": {"memory_bytes": 67108864},
            },
            "unsupported_capability",
        )
    else:
        memory_policy = {"memory_max_bytes": 67108864}
        if capabilities["memory_max"] == "hard":
            memory_policy["memory_swap_max_bytes"] = 0
        create_session(
            path,
            "memory-limit",
            program="python3",
            arguments=[
                "-c",
                "import time; b=bytearray(128*1024*1024); print(len(b),flush=True); time.sleep(10)",
            ],
            resource_policy=memory_policy,
            require_running=False,
        )
        memory_status = wait_session_exit(path, "memory-limit")
        assert (
            memory_status.get("exit_code") != 0
            or memory_status.get("exit_signal") is not None
        ), memory_status
        assert memory_status["resource_metrics"]["tier"] == (
            "hard" if capabilities["memory_max"] == "hard" else "monitored"
        ), memory_status
        assert (
            memory_status["resource_metrics"]["last_violation"] == "memory_max"
        ), memory_status

    created = create_session(
        path,
        "cpu-limit",
        arguments=["-c", "while :; do :; done"],
        resource_policy={"cpu_seconds": 1},
    )
    status = wait_session_exit(path, "cpu-limit")
    assert (
        status.get("exit_signal") is not None or status.get("exit_code") != 0
    ), status
    assert status["resource_metrics"]["last_violation"] == "cpu_seconds", status
    assert created["resource_policy"]["cpu_seconds"] == 1, created

    if capabilities["memory_high"] == "unsupported":
        expect_error(
            path,
            "SESSION_CREATE",
            {
                "session_id": "memory-high",
                "program": "sh",
                "args": [],
                "resource_policy": {"memory_high_bytes": 67108864},
            },
            "unsupported_capability",
        )
    if capabilities["cpu_quota"] == "unsupported":
        expect_error(
            path,
            "SESSION_CREATE",
            {
                "session_id": "cpu-quota",
                "program": "sh",
                "args": [],
                "resource_policy": {"cpu_quota_percent": 50},
            },
            "unsupported_capability",
        )

    dynamic = create_session(
        path, "dynamic-resource", arguments=["-c", "sleep 30"]
    )
    generation = claim(path, "dynamic-resource")
    dynamic_policy = {
        "memory_max_bytes": 33554432,
        "tasks_max": 8,
    }
    if capabilities["memory_high"] == "hard":
        dynamic_policy["memory_high_bytes"] = 16777216
    if capabilities["cpu_quota"] == "hard":
        dynamic_policy["cpu_quota_percent"] = 50
    updated = request(
        path,
        "SESSION_RESOURCE_UPDATE",
        {
            "session_id": "dynamic-resource",
            "owner": "stage4-owner",
            "generation": generation,
            "resource_policy": dynamic_policy,
        },
    )
    assert updated["resource_policy"]["memory_max_bytes"] == 33554432, updated
    assert updated["resource_policy"]["tasks_max"] == 8, updated
    if capabilities["cpu_quota"] == "hard":
        assert updated["resource_policy"]["cpu_quota_percent"] == 50, updated
    status = request(path, "SESSION_STATUS", {"session_id": "dynamic-resource"})
    assert status["resource_policy"]["tasks_max"] == 8, status
    assert status["resource_metrics"]["tier"] == (
        "hard" if capabilities["memory_max"] == "hard" else "monitored"
    ), status
    cleared = request(
        path,
        "SESSION_RESOURCE_UPDATE",
        {
            "session_id": "dynamic-resource",
            "owner": "stage4-owner",
            "generation": generation,
            "resource_policy": {
                "cpu_quota_percent": None,
                "memory_high_bytes": None,
                "memory_max_bytes": None,
                "tasks_max": None,
            },
        },
    )
    assert cleared["resource_policy"]["memory_max_bytes"] is None, cleared
    request(
        path,
        "SESSION_KILL",
        {
            "session_id": "dynamic-resource",
            "owner": "stage4-owner",
            "generation": generation,
        },
    )
    assert_process_gone(dynamic["pid"])


def probe_security(path):
    expect_closed_connection(path, struct.pack(">I", 4) + b"nope")
    expect_closed_connection(path, struct.pack(">I", MAX_FRAME_BYTES + 1))
    oversized = "x" * (MAX_INPUT_BYTES + 1)
    expect_error(
        path,
        "SESSION_INPUT",
        {"session_id": "oversized", "owner": "stage4-owner", "data": oversized},
        "input_too_large",
    )
    assert request(path, "PING", {})["daemon"] == "ok"


def probe_dynamic_stress(path):
    capabilities = request(path, "CONTROL_STATUS", {})["capabilities"]["resources"]
    initial_policy = {"memory_max_bytes": 268435456}
    if capabilities["memory_high"] == "hard":
        initial_policy["memory_high_bytes"] = 134217728
    if capabilities["memory_max"] == "hard":
        initial_policy["memory_swap_max_bytes"] = 0
    memory = create_session(
        path,
        "dynamic-memory-stress",
        program="python3",
        arguments=[
            "-c",
            (
                "import time\n"
                "b=bytearray(128*1024*1024)\n"
                "for i in range(0,len(b),4096): b[i]=1\n"
                "print('MEMORY_READY',flush=True)\n"
                "time.sleep(30)"
            ),
        ],
        resource_policy=initial_policy,
    )
    generation = claim(path, "dynamic-memory-stress")
    time.sleep(0.4)
    before = request(
        path, "SESSION_STATUS", {"session_id": "dynamic-memory-stress"}
    )
    assert before["running"] is True, before
    assert before["resource_metrics"]["memory_current_bytes"] > 67108864, before
    memory_update = request(
        path,
        "SESSION_RESOURCE_UPDATE",
        {
            "session_id": "dynamic-memory-stress",
            "owner": "stage4-owner",
            "generation": generation,
            "resource_policy": {"memory_max_bytes": 67108864},
        },
    )
    assert memory_update["resource_policy"]["memory_max_bytes"] == 67108864, (
        memory_update
    )
    updated_high = memory_update["resource_policy"]["memory_high_bytes"]
    assert updated_high is None or updated_high <= 67108864, memory_update
    memory_status = wait_session_exit(path, "dynamic-memory-stress")
    assert memory_status["resource_metrics"]["last_violation"] == "memory_max", (
        memory_status
    )
    assert_process_gone(memory["pid"])

    tasks = create_session(
        path,
        "dynamic-tasks-stress",
        program="python3",
        arguments=[
            "-c",
            (
                "import os,time\n"
                "children=[]\n"
                "for _ in range(4):\n"
                " pid=os.fork()\n"
                " if pid==0: time.sleep(30); os._exit(0)\n"
                " children.append(pid)\n"
                "print('TASKS_READY',flush=True)\n"
                "while True:\n"
                " time.sleep(0.2)\n"
                " try:\n"
                "  pid=os.fork()\n"
                "  if pid==0: os._exit(0)\n"
                "  os.waitpid(pid,0)\n"
                " except OSError:\n"
                "  print('TASK_FORK_DENIED',flush=True)\n"
                "  break\n"
                "time.sleep(10)"
            ),
        ],
        resource_policy={"tasks_max": 20},
    )
    generation = claim(path, "dynamic-tasks-stress")
    wait_session_snapshot(path, "dynamic-tasks-stress", "TASKS_READY")
    request(
        path,
        "SESSION_RESOURCE_UPDATE",
        {
            "session_id": "dynamic-tasks-stress",
            "owner": "stage4-owner",
            "generation": generation,
            "resource_policy": {"tasks_max": 3},
        },
    )
    tasks_status = request(
        path, "SESSION_STATUS", {"session_id": "dynamic-tasks-stress"}
    )
    if capabilities["tasks_max"] == "hard_tasks":
        wait_session_snapshot(path, "dynamic-tasks-stress", "TASK_FORK_DENIED")
        assert tasks_status["running"] is True, tasks_status
    else:
        tasks_status = wait_session_exit(path, "dynamic-tasks-stress")
        assert tasks_status["resource_metrics"]["last_violation"] == "tasks_max", (
            tasks_status
        )
    request(
        path,
        "SESSION_KILL",
        {
            "session_id": "dynamic-tasks-stress",
            "owner": "stage4-owner",
            "generation": generation,
        },
    )
    assert_process_gone(tasks["pid"])


def probe_teardown(path):
    single = create_session(
        path, "kill-session", arguments=["-c", "sleep 30"]
    )
    generation = claim(path, "kill-session")
    killed = request(
        path,
        "SESSION_KILL",
        {
            "session_id": "kill-session",
            "owner": "stage4-owner",
            "generation": generation,
        },
    )
    assert killed["running"] is False, killed
    assert_process_gone(single["pid"])

    created = []
    for session_id in ("batch-a", "batch-b"):
        item = create_session(path, session_id, arguments=["-c", "sleep 30"])
        claim(path, session_id)
        created.append(item)
    batch = request(
        path,
        "TASK_KILL_BATCH",
        {
            "task_ids": [item["task_id"] for item in created],
            "owner": "stage4-owner",
            "generation": 1,
        },
    )
    assert len(batch["tasks"]) == 2, batch
    assert batch["failures"] == [], batch
    for item in created:
        assert_process_gone(item["pid"])

    status = request(path, "CONTROL_STATUS", {})
    assert any(
        session["id"] == "kill-session" and session["state"] == "closed"
        for session in status["sessions"]
    ), status

    expect_error(path, "SERVER_KILL", {}, "confirmation_required")
    stopping = request(path, "SERVER_KILL", {"confirm": True})
    assert stopping["daemon"] == "stopping", stopping
    assert stopping["failures"] == [], stopping
    return created


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    qwed = None
    worker_pids = []
    with tempfile.TemporaryDirectory(prefix="qwe-stage4-") as root:
        root = Path(root)
        credential_dir = root / "credentials"
        credential_dir.mkdir(mode=0o700)
        key_path = credential_dir / "STAGE4_KEY"
        key_path.write_bytes(SECRET)
        key_path.chmod(0o600)
        try:
            qwed = start_qwed(args.qwed, args.socket, str(credential_dir))
            credential = probe_credentials(args.socket, qwed, credential_dir)
            worker_pids.extend([credential["worker_pid"], credential["pid"]])
            probe_resources(args.socket)
            probe_dynamic_stress(args.socket)
            probe_security(args.socket)
            worker_pids.extend(item["pid"] for item in probe_teardown(args.socket))
            finished_qwed = qwed
            finished_qwed.wait(timeout=5)
            assert finished_qwed.returncode == 0, finished_qwed.returncode
            stderr = finished_qwed.stderr.read()
            qwed = None

            database = Path(f"{args.socket}.sqlite")
            evidence = [
                database,
                Path(f"{args.socket}.sqlite-shm"),
                Path(f"{args.socket}.sqlite-wal"),
            ]
            for path in evidence:
                if path.exists():
                    assert SECRET not in path.read_bytes(), f"secret leaked to {path}"
            assert SECRET_PREFIX.encode() not in stderr
            print(
                json.dumps(
                    {
                        "case": "CRED-001/RES-001/RES-003/RES-004/RES-005/RES-006/RES-007/SEC-001/KILL-001",
                        "credential_descriptor": "pass",
                        "credential_redaction": "pass",
                        "resource_capability": "pass",
                        "malformed_frame": "pass",
                        "oversized_input": "pass",
                        "session_teardown": "pass",
                        "batch_task_teardown": "pass",
                        "server_teardown": "pass",
                    },
                    separators=(",", ":"),
                )
            )
        finally:
            if qwed is not None:
                stop_qwed(qwed)
            for pid in worker_pids:
                if isinstance(pid, int):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass


if __name__ == "__main__":
    main()
