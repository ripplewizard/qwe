#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import subprocess
import time
from pathlib import Path

from stage7_resource_probe import (
    request,
    request_with_capability,
    wait_for,
)


OWNER = "systemd-service-smoke"


def wait_qwed(socket_path, timeout=15):
    def ready():
        try:
            result = request(socket_path, "PING", {})
        except (AssertionError, ConnectionError, OSError):
            return None
        return result if result.get("daemon") == "ok" else None

    return wait_for(ready, "qwed did not become ready", timeout)


def create_session(socket_path, session_id, workspace):
    created = request(
        socket_path,
        "SESSION_CREATE",
        {
            "session_id": session_id,
            "program": "/bin/sh",
            "args": ["-c", "exec /bin/sleep 120"],
            "owner": OWNER,
        },
    )
    assert created.get("running") is True, created
    return created["pane_id"]


def issue_capability(socket_path, management, principal_id, pane_id, workspace):
    return request_with_capability(
        socket_path,
        "SECURITY_CAPABILITY_ISSUE",
        {
            "principal_id": principal_id,
            "operation": "agent_exec",
            "scope": {"kind": "pane", "pane_id": pane_id},
            "ttl_ms": 60000,
            "sandbox_profile": {
                "version": 1,
                "workspace": str(workspace),
                "workspace_mode": "read_only",
                "network": "isolated_loopback",
                "home": "hidden",
                "private_tmp": True,
                "process_namespace": "isolated",
                "seccomp": {"mode": "none"},
            },
        },
        management,
    )


def exec_sandbox(socket_path, capability, pane_id, script):
    return request_with_capability(
        socket_path,
        "AGENT_EXEC",
        {
            "pane_id": pane_id,
            "program": "/bin/sh",
            "args": ["-c", script],
            "owner": OWNER,
        },
        capability,
    )


def wait_status(socket_path, capability, command_id, timeout=8):
    def started():
        status = request_with_capability(
            socket_path,
            "AGENT_COMMAND_STATUS",
            {"command_id": command_id},
            capability,
        )
        return status if status.get("process_identity") else None

    return wait_for(started, "sandboxed command did not start", timeout)


def wait_terminal(socket_path, capability, command_id, timeout=8):
    def terminal():
        result = request_with_capability(
            socket_path,
            "AGENT_COMMAND_WAIT_IDLE",
            {"command_id": command_id, "timeout_ms": 1000},
            capability,
        )
        return result if result["wait_state"] in ("done", "failed", "lost") else None

    return wait_for(terminal, "sandboxed command did not terminate", timeout)


def output(socket_path, capability, command_id):
    result = request_with_capability(
        socket_path,
        "AGENT_COMMAND_EVENTS",
        {"command_id": command_id, "cursor": 0, "limit": 512},
        capability,
    )
    chunks = []
    cursor = 0
    for event in result["events"]:
        if event["type"] == "output" and event["stream"] == "stdout":
            data = bytes.fromhex(event["data_hex"])
            assert event["cursor"] == cursor, event
            cursor += len(data)
            chunks.append(data)
    return b"".join(chunks)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--unit", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--service-bin-dir", required=True)
    parser.add_argument("--release-bin-dir", required=True)
    args = parser.parse_args()

    workspace = Path(args.workspace)
    wait_qwed(args.socket)
    management = Path(
        f"{args.socket}.runtime/security.bootstrap"
    ).read_text().strip()
    principal_id = request_with_capability(
        args.socket,
        "SECURITY_BOOTSTRAP",
        {},
        management,
    )["principal"]["principal_id"]

    smoke_panes = []
    pane_id = create_session(args.socket, "systemd-smoke-recovery", workspace)
    smoke_panes.append(pane_id)
    capability = issue_capability(
        args.socket,
        management,
        principal_id,
        pane_id,
        workspace,
    )
    assert capability.get("token"), capability
    assert capability["sandbox_audit"]["backend_path"].endswith("bwrap")
    command = exec_sandbox(
        args.socket,
        capability["token"],
        pane_id,
        "sleep 1; printf 'SERVICE_RECOVERED'",
    )
    before = wait_status(args.socket, capability["token"], command["command_id"])
    subprocess.run(
        ["systemctl", "--user", "kill", "--signal=SIGKILL", f"{args.unit}.service"],
        check=True,
    )
    wait_qwed(args.socket)
    after = wait_status(args.socket, capability["token"], command["command_id"])
    assert after["sandbox"]["audit"] == before["sandbox"]["audit"], (before, after)
    terminal = wait_terminal(args.socket, capability["token"], command["command_id"])
    assert terminal["wait_state"] == "done", terminal
    assert output(args.socket, capability["token"], command["command_id"]) == (
        b"SERVICE_RECOVERED"
    )

    staged = Path(args.service_bin_dir) / f".qwed-{os.getpid()}"
    shutil.copy2(Path(args.release_bin_dir) / "qwed", staged)
    os.replace(staged, Path(args.service_bin_dir) / "qwed")
    subprocess.run(
        ["systemctl", "--user", "restart", f"{args.unit}.service"],
        check=True,
    )
    wait_qwed(args.socket)
    management = Path(
        f"{args.socket}.runtime/security.bootstrap"
    ).read_text().strip()
    principal_id = request_with_capability(
        args.socket,
        "SECURITY_BOOTSTRAP",
        {},
        management,
    )["principal"]["principal_id"]
    pane_id = create_session(args.socket, "systemd-smoke-upgrade", workspace)
    smoke_panes.append(pane_id)
    capability = issue_capability(
        args.socket,
        management,
        principal_id,
        pane_id,
        workspace,
    )["token"]
    upgraded = exec_sandbox(
        args.socket,
        capability,
        pane_id,
        "printf 'SERVICE_UPGRADE_OK'",
    )
    terminal = wait_terminal(args.socket, capability, upgraded["command_id"])
    assert terminal["wait_state"] == "done", terminal
    assert output(args.socket, capability, upgraded["command_id"]) == (
        b"SERVICE_UPGRADE_OK"
    )

    subprocess.run(
        ["systemctl", "--user", "stop", f"{args.unit}.service"],
        check=True,
    )
    active_state = subprocess.run(
        [
            "systemctl",
            "--user",
            "show",
            f"{args.unit}.service",
            "--property=ActiveState",
            "--value",
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    assert active_state in ("inactive", "failed"), active_state
    for pane_id in smoke_panes:
        shown = subprocess.run(
            [
                "systemctl",
                "--user",
                "show",
                f"qwe-pane-{pane_id}.scope",
                "--property=ActiveState",
                "--value",
            ],
            capture_output=True,
            text=True,
        )
        if shown.returncode == 0:
            assert shown.stdout.strip() not in ("active", "reloading"), shown.stdout

    print(
        json.dumps(
            {
                "case": "SANDBOX-SYSTEMD-SERVICE-001",
                "status": "pass",
                "sandbox_agent": "pass",
                "qwed_restart_recovery": "pass",
                "binary_replacement": "pass",
                "upgrade_exec": "pass",
                "teardown": "pass",
            }
        )
    )


if __name__ == "__main__":
    main()
