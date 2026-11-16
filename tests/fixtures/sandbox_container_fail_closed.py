#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import tempfile
from pathlib import Path

from stage7_resource_probe import (
    request,
    request_with_capability,
    start_qwed,
    stop_qwed,
)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--matrix", default="container")
    parser.add_argument("--expected-reason", default="mount_denied")
    parser.add_argument("--expected-target")
    args = parser.parse_args()

    bwrap = shutil.which("bwrap")
    assert bwrap is not None, "container matrix requires an installed Bubblewrap"
    root = Path(tempfile.mkdtemp(prefix="qwe-sandbox-matrix-"))
    workspace = root / "workspace"
    workspace.mkdir()
    socket_path = str(root / "qwed.sock")
    qwed = start_qwed(
        args.qwed,
        socket_path,
        root / "qwed.log",
    )
    try:
        management = Path(
            f"{socket_path}.runtime/security.bootstrap"
        ).read_text().strip()
        principal_id = request_with_capability(
            socket_path,
            "SECURITY_BOOTSTRAP",
            {},
            management,
        )["principal"]["principal_id"]
        created = request(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": "sandbox-matrix",
                "program": "/bin/sh",
                "args": ["-c", "exec /bin/sleep 120"],
                "owner": "sandbox-matrix",
            },
        )
        assert created.get("running") is True, created
        result = request_with_capability(
            socket_path,
            "SECURITY_CAPABILITY_ISSUE",
            {
                "principal_id": principal_id,
                "operation": "agent_exec",
                "scope": {"kind": "pane", "pane_id": created["pane_id"]},
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
        assert result.get("code") == "sandbox_backend_unavailable", result
        assert result["details"]["reason"] == args.expected_reason, result
        if args.expected_target is not None:
            assert (
                result["details"]["diagnostics"]["target"]
                == args.expected_target
            ), result
        unsandboxed = request_with_capability(
            socket_path,
            "SECURITY_CAPABILITY_ISSUE",
            {
                "principal_id": principal_id,
                "operation": "agent_exec",
                "scope": {"kind": "pane", "pane_id": created["pane_id"]},
                "ttl_ms": 60000,
            },
            management,
        )
        started = request_with_capability(
            socket_path,
            "AGENT_EXEC",
            {
                "pane_id": created["pane_id"],
                "program": "/bin/sh",
                "args": ["-c", "printf NORMAL_PANE_OK"],
                "owner": "sandbox-matrix",
            },
            unsandboxed["token"],
        )
        terminal = request_with_capability(
            socket_path,
            "AGENT_COMMAND_WAIT_IDLE",
            {"command_id": started["command_id"], "timeout_ms": 5000},
            unsandboxed["token"],
        )
        assert terminal["wait_state"] == "done", terminal
        assert terminal["exit_code"] == 0, terminal
        command_events = request_with_capability(
            socket_path,
            "AGENT_COMMAND_EVENTS",
            {"command_id": started["command_id"], "cursor": 0, "limit": 512},
            unsandboxed["token"],
        )["events"]
        output = b"".join(
            bytes.fromhex(event["data_hex"])
            for event in command_events
            if event["type"] == "output" and event["stream"] == "stdout"
        )
        assert output == b"NORMAL_PANE_OK", command_events
    finally:
        stop_qwed(qwed, socket_path)

    print(
        json.dumps(
            {
                "case": "SANDBOX-004/SANDBOX-015",
                "matrix": args.matrix,
                "image": args.image,
                "bwrap": bwrap,
                "status": "pass",
                "behavior": "sandbox_backend_unavailable",
                "ordinary_pane": "pass",
            }
        )
    )


if __name__ == "__main__":
    main()
