#!/usr/bin/env python3

import argparse
import json
import os
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from stage7_resource_probe import (
    crash_qwed,
    request,
    request_with_capability,
    start_qwed,
    stop_qwed,
    wait_for,
)


OWNER = "stage9-agent"
SESSION_ID = "stage9-agent-session"
COMMAND_SECRET = b"stage9-command-credential-secret-7f61c4"


def wait_result(socket_path, command_id, capability, timeout_ms=5000):
    return request_with_capability(
        socket_path,
        "AGENT_COMMAND_WAIT_IDLE",
        {"command_id": command_id, "timeout_ms": timeout_ms},
        capability,
    )


def exec_command(
    socket_path,
    capability,
    pane_id,
    program,
    *arguments,
    deadline_ms=None,
    credential=None,
    pty=False,
):
    payload = {
        "pane_id": pane_id,
        "program": program,
        "args": list(arguments),
        "owner": OWNER,
    }
    if deadline_ms is not None:
        payload["deadline_ms"] = deadline_ms
    if credential is not None:
        payload["credential"] = credential
    if pty:
        payload["pty"] = True
        payload["rows"] = 24
        payload["columns"] = 80
    return request_with_capability(socket_path, "AGENT_EXEC", payload, capability)


def output_events(socket_path, command_id, capability):
    result = request_with_capability(
        socket_path,
        "AGENT_COMMAND_EVENTS",
        {"command_id": command_id, "cursor": 0, "limit": 512},
        capability,
    )
    assert result["replay_gap"] is None, result
    return result["events"]


def decode_stream(events, stream):
    chunks = []
    previous_cursor = 0
    for event in events:
        if event["type"] != "output" or event["stream"] != stream:
            continue
        assert event["cursor"] == previous_cursor, event
        data = bytes.fromhex(event["data_hex"])
        previous_cursor += len(data)
        chunks.append(data)
    return b"".join(chunks)


def cgroup_path(pid):
    for line in Path(f"/proc/{pid}/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            return line[3:]
    raise RuntimeError(f"PID {pid} has no cgroup v2 path")


def cli_json(cli, socket_path, *arguments):
    completed = subprocess.run(
        [cli, "--socket", socket_path, "--owner", OWNER, *arguments],
        check=True,
        capture_output=True,
        text=True,
        timeout=15,
    )
    return json.loads(completed.stdout)


def started_agent_status(socket_path, command_id, capability):
    status = request_with_capability(
        socket_path,
        "AGENT_COMMAND_STATUS",
        {"command_id": command_id},
        capability,
    )
    return status if status.get("process_identity") else None


def lost_agent_status(socket_path, command_id, capability):
    try:
        status = request_with_capability(
            socket_path,
            "AGENT_COMMAND_STATUS",
            {"command_id": command_id},
            capability,
        )
    except (AssertionError, ConnectionError, OSError):
        return None
    return status if status.get("state") == "lost" else None


def bootstrap_management(socket_path):
    path = Path(f"{socket_path}.runtime/security.bootstrap")
    return path, path.read_text().strip()


def issue_capability(
    socket_path,
    management,
    principal_id,
    operation,
    scope,
    ttl_ms=60000,
):
    return request_with_capability(
        socket_path,
        "SECURITY_CAPABILITY_ISSUE",
        {
            "principal_id": principal_id,
            "operation": operation,
            "scope": scope,
            "ttl_ms": ttl_ms,
        },
        management,
    )


def create_principal(socket_path, management, parent_id, high_trust=False):
    return request_with_capability(
        socket_path,
        "SECURITY_PRINCIPAL_CREATE",
        {"parent_id": parent_id, "high_trust": high_trust},
        management,
    )


def revoke_principal(socket_path, management, principal_id, **options):
    return request_with_capability(
        socket_path,
        "SECURITY_PRINCIPAL_REVOKE",
        {"principal_id": principal_id, **options},
        management,
    )


def receive_exact(stream, count):
    chunks = []
    while count:
        chunk = stream.recv(count)
        if not chunk:
            raise EOFError("observer socket closed")
        chunks.append(chunk)
        count -= len(chunk)
    return b"".join(chunks)


def observe_command(socket_path, capability, command_id, timeout_ms=5000):
    request = {
        "version": 1,
        "request_id": f"observe-{time.time_ns()}",
        "operation": "AGENT_COMMAND_OBSERVE",
        "payload": {
            "command_id": command_id,
            "cursor": 0,
            "timeout_ms": timeout_ms,
        },
        "auth_context": {"capability": capability},
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    frames = []
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(10)
        stream.connect(socket_path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        while True:
            length = struct.unpack(">I", receive_exact(stream, 4))[0]
            response = json.loads(receive_exact(stream, length))
            result = response["result"]
            value = result.get("Ok", result.get("Err"))
            frames.append(value)
            if value.get("type") == "end" or value.get("type") == "error":
                break
    return frames


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", required=True)
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    qwed = None
    log_path = Path(tempfile.gettempdir()) / f"qwe-stage9-agent-{os.getpid()}.log"
    try:
        credentials_dir = Path(args.socket).parent / "credentials"
        credentials_dir.mkdir(mode=0o700)
        credential_path = credentials_dir / "CMD_KEY"
        credential_path.write_bytes(COMMAND_SECRET)
        credential_path.chmod(0o600)
        empty_credential_path = credentials_dir / "EMPTY_KEY"
        empty_credential_path.write_bytes(b"")
        empty_credential_path.chmod(0o600)
        qwed = start_qwed(
            args.qwed,
            args.socket,
            log_path,
            credentials_dir=credentials_dir,
        )
        bootstrap_path, management = bootstrap_management(args.socket)
        assert stat.S_IMODE(bootstrap_path.stat().st_mode) == 0o600
        bootstrap = request_with_capability(
            args.socket,
            "SECURITY_BOOTSTRAP",
            {},
            management,
        )
        root_principal = bootstrap["principal"]["principal_id"]
        cli_bootstrap = cli_json(
            args.cli,
            args.socket,
            "security",
            "bootstrap",
        )
        assert cli_bootstrap["principal"]["principal_id"] == root_principal
        created = request(
            args.socket,
            "SESSION_CREATE",
            {
                "session_id": SESSION_ID,
                "program": "sh",
                "args": ["-c", "exec sleep 120"],
                "owner": OWNER,
            },
        )
        assert created.get("running") is True, created
        pane_id = created["pane_id"]
        exec_capability = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_exec",
            {"kind": "pane", "pane_id": pane_id},
        )["token"]
        started = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sh",
            "-c",
            "printf 'STDOUT_MARKER'; printf 'STDERR_MARKER' >&2; exit 7",
        )
        command_id = started["command_id"]
        terminal = wait_result(args.socket, command_id, exec_capability)
        assert terminal["wait_state"] == "done", terminal
        assert terminal["exit_code"] == 7, terminal
        first_page = request_with_capability(
            args.socket,
            "AGENT_COMMAND_EVENTS",
            {"command_id": command_id, "cursor": 0, "limit": 2},
            exec_capability,
        )
        second_page = request_with_capability(
            args.socket,
            "AGENT_COMMAND_EVENTS",
            {"command_id": command_id, "cursor": first_page["next_cursor"], "limit": 2},
            exec_capability,
        )
        assert first_page["events"], first_page
        assert second_page["events"], second_page
        assert second_page["events"][0]["sequence"] > first_page["events"][-1]["sequence"]
        events = output_events(args.socket, command_id, exec_capability)
        assert decode_stream(events, "stdout") == b"STDOUT_MARKER", events
        assert decode_stream(events, "stderr") == b"STDERR_MARKER", events
        assert any(event["type"] == "terminal_result" for event in events), events

        flooding = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sh",
            "-c",
            "dd if=/dev/zero bs=65536 count=20 status=none",
        )
        flooding_terminal = wait_result(
            args.socket,
            flooding["command_id"],
            exec_capability,
        )
        assert flooding_terminal["wait_state"] == "done", flooding_terminal
        bounded = request_with_capability(
            args.socket,
            "AGENT_COMMAND_EVENTS",
            {"command_id": flooding["command_id"], "cursor": 0, "limit": 512},
            exec_capability,
        )
        assert bounded["replay_gap"] is not None, bounded["replay_from"]
        assert len(bounded["events"]) < 40, len(bounded["events"])

        scope_check = "not_applicable"
        if sys.platform.startswith("linux"):
            scoped = exec_command(
                args.socket,
                exec_capability,
                pane_id,
                "/bin/sleep",
                "5",
            )
            scoped_status = wait_for(
                lambda: started_agent_status(
                    args.socket, scoped["command_id"], exec_capability
                ),
                "agent process identity did not become available",
            )
            pane_status = request(args.socket, "PANE_STATUS", {"pane_id": pane_id})
            command_pid = scoped_status["process_identity"]["pid"]
            worker_pid = pane_status["worker_pid"]
            shell_pid = pane_status["pid"]
            command_cgroup = Path(cgroup_path(command_pid))
            worker_cgroup = Path(cgroup_path(worker_pid))
            shell_cgroup = Path(cgroup_path(shell_pid))
            assert command_cgroup == shell_cgroup, (command_cgroup, shell_cgroup)
            assert command_cgroup.parent == worker_cgroup.parent, (
                command_cgroup,
                worker_cgroup,
            )
            request_with_capability(
                args.socket,
                "AGENT_COMMAND_TERMINATE",
                {"command_id": scoped["command_id"]},
                exec_capability,
            )
            wait_result(args.socket, scoped["command_id"], exec_capability, 3000)
            scope_check = "pass"

        signaled = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sh",
            "-c",
            "kill -TERM $$",
        )
        signaled_result = wait_result(
            args.socket, signaled["command_id"], exec_capability
        )
        assert signaled_result["wait_state"] == "failed", signaled_result
        assert signaled_result["exit_signal"] == "SIGTERM", signaled_result

        missing = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/definitely/not/a/qwe-program",
        )
        missing_result = wait_result(args.socket, missing["command_id"], exec_capability)
        assert missing_result["wait_state"] == "failed", missing_result

        expiring = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sleep",
            "5",
            deadline_ms=int(time.time() * 1000) + 150,
        )
        expiring_result = wait_result(
            args.socket, expiring["command_id"], exec_capability, 3000
        )
        assert expiring_result["wait_state"] == "failed", expiring_result
        assert expiring_result["exit_signal"] == "deadline_exceeded", expiring_result

        pending = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sleep",
            "5",
        )
        unknown = wait_result(args.socket, pending["command_id"], exec_capability, 50)
        assert unknown["wait_state"] == "unknown", unknown
        assert unknown["timed_out"] is True, unknown
        request_with_capability(
            args.socket,
            "AGENT_COMMAND_TERMINATE",
            {"command_id": pending["command_id"]},
            exec_capability,
        )
        terminated = wait_result(
            args.socket, pending["command_id"], exec_capability, 3000
        )
        assert terminated["wait_state"] == "failed", terminated

        immediate = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sleep",
            "5",
        )
        request_with_capability(
            args.socket,
            "AGENT_COMMAND_TERMINATE",
            {"command_id": immediate["command_id"]},
            exec_capability,
        )
        immediate_result = wait_result(
            args.socket, immediate["command_id"], exec_capability, 3000
        )
        assert immediate_result["wait_state"] == "failed", immediate_result

        credential_script = (
            "import os,time;"
            "secret=os.read(3,16384);"
            "os.write(1,b'STDOUT:');"
            "os.write(1,secret[:8]);"
            "time.sleep(0.15);"
            "os.write(1,secret[8:]+b'\\n');"
            "time.sleep(0.15);"
            "os.write(2,b'STDERR:'+secret+b'\\n');"
            "leak=any(secret.decode() in value for value in os.environ.values());"
            "print(f'ENV_SECRET={int(leak)}',flush=True);"
            "time.sleep(0.5)"
        )
        credentialed = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/usr/bin/python3",
            "-c",
            credential_script,
            credential="CMD_KEY",
        )
        credentialed_status = wait_for(
            lambda: started_agent_status(
                args.socket,
                credentialed["command_id"],
                exec_capability,
            ),
            "credentialed command did not start",
        )
        if sys.platform.startswith("linux"):
            command_pid = credentialed_status["process_identity"]["pid"]
            command_line = Path(f"/proc/{command_pid}/cmdline").read_bytes()
            command_environment = Path(f"/proc/{command_pid}/environ").read_bytes()
            assert COMMAND_SECRET not in command_line, command_line
            assert COMMAND_SECRET not in command_environment, command_environment
            assert b"QWE_CREDENTIAL_FD=3" in command_environment, command_environment
        credentialed_terminal = wait_result(
            args.socket,
            credentialed["command_id"],
            exec_capability,
            4000,
        )
        assert credentialed_terminal["wait_state"] == "done", credentialed_terminal
        credentialed_events = output_events(
            args.socket,
            credentialed["command_id"],
            exec_capability,
        )
        credentialed_stdout = decode_stream(credentialed_events, "stdout")
        credentialed_stderr = decode_stream(credentialed_events, "stderr")
        assert credentialed_stdout == b"STDOUT:[REDACTED]\nENV_SECRET=0\n", (
            credentialed_stdout
        )
        assert credentialed_stderr == b"STDERR:[REDACTED]\n", credentialed_stderr
        assert any(
            event["type"] == "redaction_applied"
            and event["policy"] == "credential_exact"
            for event in credentialed_events
        ), credentialed_events

        missing_credential = request_with_capability(
            args.socket,
            "AGENT_EXEC",
            {
                "pane_id": pane_id,
                "program": "/bin/true",
                "args": [],
                "owner": OWNER,
                "credential": "MISSING_KEY",
            },
            exec_capability,
        )
        assert missing_credential["code"] == "credential_unavailable", (
            missing_credential
        )

        empty_credential = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/true",
            credential="EMPTY_KEY",
        )
        empty_result = wait_result(
            args.socket,
            empty_credential["command_id"],
            exec_capability,
            3000,
        )
        assert empty_result["wait_state"] == "failed", empty_result
        assert "credential is empty" in empty_result["error"], empty_result

        pty_script = (
            "import os,time;"
            "secret=os.read(3,16384);"
            "os.write(1,b'\\x1b[32m');"
            "os.write(1,secret[:6]);"
            "time.sleep(0.1);"
            "os.write(1,secret[6:]);"
            "os.write(1,b'\\x1b[0m\\n')"
        )
        pty_command = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/usr/bin/python3",
            "-c",
            pty_script,
            credential="CMD_KEY",
            pty=True,
        )
        pty_terminal = wait_result(
            args.socket,
            pty_command["command_id"],
            exec_capability,
            4000,
        )
        assert pty_terminal["wait_state"] == "done", pty_terminal
        pty_events = output_events(
            args.socket,
            pty_command["command_id"],
            exec_capability,
        )
        assert decode_stream(pty_events, "pty") == b"\x1b[32m[REDACTED]\x1b[0m\r\n", (
            decode_stream(pty_events, "pty")
        )

        idle_shell = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sh",
            pty=True,
        )
        idle_result = wait_result(
            args.socket,
            idle_shell["command_id"],
            exec_capability,
            3000,
        )
        assert idle_result["wait_state"] == "idle", idle_result
        idle_status = request_with_capability(
            args.socket,
            "AGENT_COMMAND_STATUS",
            {"command_id": idle_shell["command_id"]},
            exec_capability,
        )
        assert idle_status["state"] == "idle", idle_status
        idle_events = output_events(
            args.socket,
            idle_shell["command_id"],
            exec_capability,
        )
        assert any(event["type"] == "idle_observed" for event in idle_events), (
            idle_events
        )
        request_with_capability(
            args.socket,
            "AGENT_COMMAND_TERMINATE",
            {"command_id": idle_shell["command_id"]},
            exec_capability,
        )
        wait_result(
            args.socket,
            idle_shell["command_id"],
            exec_capability,
            3000,
        )

        observer_principal = create_principal(
            args.socket, management, root_principal
        )["principal_id"]
        observed_command = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/usr/bin/python3",
            "-c",
            (
                "import time;"
                "print('OBS_ONE',flush=True);"
                "time.sleep(0.35);"
                "print('OBS_TWO',flush=True)"
            ),
        )
        observer_capability = issue_capability(
            args.socket,
            management,
            observer_principal,
            "agent_command_read",
            {
                "kind": "command",
                "pane_id": pane_id,
                "command_id": observed_command["command_id"],
            },
        )["token"]
        observer_frames = observe_command(
            args.socket,
            observer_capability,
            observed_command["command_id"],
        )
        assert len(observer_frames) >= 2, observer_frames
        assert observer_frames[-1]["type"] == "end", observer_frames[-1]
        observed_events = [
            event
            for frame in observer_frames
            if frame.get("type") == "batch"
            for event in frame.get("events", [])
        ]
        observed_stdout = decode_stream(observed_events, "stdout")
        assert observed_stdout == b"OBS_ONE\nOBS_TWO\n", observed_stdout
        observer_denied = request_with_capability(
            args.socket,
            "AGENT_COMMAND_TERMINATE",
            {"command_id": observed_command["command_id"]},
            observer_capability,
        )
        assert observer_denied["code"] == "capability_scope_denied", observer_denied
        observer_capability_path = Path(args.socket).parent / "observer.capability"
        observer_capability_path.write_text(observer_capability + "\n")
        observer_capability_path.chmod(0o600)
        cli_observer = subprocess.run(
            [
                args.cli,
                "--socket",
                args.socket,
                "--capability-file",
                str(observer_capability_path),
                "agent",
                "observe",
                observed_command["command_id"],
                "--timeout-ms",
                "1000",
            ],
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        )
        cli_observer_frames = [
            json.loads(line) for line in cli_observer.stdout.splitlines() if line
        ]
        assert any(frame.get("type") == "batch" for frame in cli_observer_frames), (
            cli_observer_frames
        )

        missing_capability = request(
            args.socket,
            "AGENT_EXEC",
            {
                "pane_id": pane_id,
                "program": "/bin/true",
                "args": [],
                "owner": OWNER,
            },
        )
        assert missing_capability["code"] == "capability_missing", missing_capability

        other_pane = request(
            args.socket,
            "SESSION_CREATE",
            {
                "session_id": "stage9-agent-other",
                "program": "sleep",
                "args": ["120"],
                "owner": OWNER,
            },
        )["pane_id"]
        cross_pane_capability = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_exec",
            {"kind": "pane", "pane_id": other_pane},
        )["token"]
        cross_pane = request_with_capability(
            args.socket,
            "AGENT_EXEC",
            {
                "pane_id": pane_id,
                "program": "/bin/true",
                "args": [],
                "owner": OWNER,
            },
            cross_pane_capability,
        )
        assert cross_pane["code"] == "capability_scope_denied", cross_pane

        expiring_capability = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_exec",
            {"kind": "pane", "pane_id": pane_id},
            ttl_ms=1,
        )["token"]
        time.sleep(0.02)
        expired = request_with_capability(
            args.socket,
            "AGENT_EXEC",
            {
                "pane_id": pane_id,
                "program": "/bin/true",
                "args": [],
                "owner": OWNER,
            },
            expiring_capability,
        )
        assert expired["code"] == "capability_expired", expired

        ancestor_parent = create_principal(
            args.socket, management, root_principal
        )["principal_id"]
        ancestor_child = create_principal(
            args.socket, management, ancestor_parent
        )["principal_id"]
        ancestor_capability = issue_capability(
            args.socket,
            management,
            ancestor_child,
            "agent_exec",
            {"kind": "pane", "pane_id": pane_id},
        )["token"]
        ancestor_command = exec_command(
            args.socket,
            ancestor_capability,
            pane_id,
            "/bin/sleep",
            "5",
        )
        ancestor_status = wait_for(
            lambda: started_agent_status(
                args.socket,
                ancestor_command["command_id"],
                ancestor_capability,
            ),
            "ancestor test command did not start",
        )
        admin_revocation = revoke_principal(
            args.socket,
            management,
            ancestor_parent,
            severity="administrative",
        )
        assert admin_revocation["principal"]["revocation"]["severity"] == (
            "administrative"
        ), admin_revocation
        ancestor_denied = request_with_capability(
            args.socket,
            "AGENT_COMMAND_STATUS",
            {"command_id": ancestor_command["command_id"]},
            ancestor_capability,
        )
        assert ancestor_denied["code"] == "principal_revoked", ancestor_denied
        os.kill(ancestor_status["process_identity"]["pid"], 0)

        root_read = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_command_read",
            {
                "kind": "command",
                "pane_id": pane_id,
                "command_id": ancestor_command["command_id"],
            },
        )["token"]
        root_terminate = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_command_terminate",
            {
                "kind": "command",
                "pane_id": pane_id,
                "command_id": ancestor_command["command_id"],
            },
        )["token"]
        assert request_with_capability(
            args.socket,
            "AGENT_COMMAND_STATUS",
            {"command_id": ancestor_command["command_id"]},
            root_read,
        )["state"] == "running"
        request_with_capability(
            args.socket,
            "AGENT_COMMAND_TERMINATE",
            {"command_id": ancestor_command["command_id"]},
            root_terminate,
        )
        wait_result(
            args.socket,
            ancestor_command["command_id"],
            root_read,
            3000,
        )

        freeze_principal = create_principal(
            args.socket, management, root_principal
        )["principal_id"]
        freeze_capability = issue_capability(
            args.socket,
            management,
            freeze_principal,
            "agent_exec",
            {"kind": "pane", "pane_id": pane_id},
        )["token"]
        frozen_command = exec_command(
            args.socket,
            freeze_capability,
            pane_id,
            "/bin/sleep",
            "5",
        )
        freeze_revocation = revoke_principal(
            args.socket,
            management,
            freeze_principal,
            grace_ms=150,
            hold=True,
        )
        assert freeze_revocation["principal"]["revocation"]["severity"] == (
            "freeze"
        ), freeze_revocation
        time.sleep(0.6)
        frozen_read = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_command_read",
            {
                "kind": "command",
                "pane_id": pane_id,
                "command_id": frozen_command["command_id"],
            },
        )["token"]
        frozen_terminal = wait_result(
            args.socket,
            frozen_command["command_id"],
            frozen_read,
            3000,
        )
        assert frozen_terminal["wait_state"] == "failed", frozen_terminal
        frozen_events = output_events(
            args.socket,
            frozen_command["command_id"],
            frozen_read,
        )
        revocations = [
            event
            for event in frozen_events
            if event["type"] == "revocation_applied"
        ]
        assert [event["severity"] for event in revocations] == [
            "freeze",
            "terminate",
        ], revocations

        low_trust_manager = create_principal(
            args.socket, management, root_principal
        )["principal_id"]
        low_trust_management = issue_capability(
            args.socket,
            management,
            low_trust_manager,
            "manage_principals",
            {"kind": "global"},
        )["token"]
        revocation_target = create_principal(
            args.socket, management, root_principal
        )["principal_id"]
        insufficient = revoke_principal(
            args.socket,
            low_trust_management,
            revocation_target,
            severity="administrative",
        )
        assert insufficient["code"] == "insufficient_trust", insufficient

        cli_principal = cli_json(
            args.cli,
            args.socket,
            "security",
            "principal-create",
            root_principal,
        )["principal_id"]
        cli_capability_path = Path(args.socket).parent / "cli-agent.capability"
        cli_issue = cli_json(
            args.cli,
            args.socket,
            "security",
            "capability-issue",
            cli_principal,
            "agent_exec",
            "--pane-id",
            pane_id,
            "--output",
            str(cli_capability_path),
        )
        assert "token" not in cli_issue, cli_issue
        assert stat.S_IMODE(cli_capability_path.stat().st_mode) == 0o600
        insecure_capability_path = Path(args.socket).parent / "insecure.capability"
        insecure_capability_path.write_text(cli_capability_path.read_text())
        insecure_capability_path.chmod(0o644)
        insecure_cli = subprocess.run(
            [
                args.cli,
                "--socket",
                args.socket,
                "--capability-file",
                str(insecure_capability_path),
                "agent",
                "status",
                "not-used",
            ],
            capture_output=True,
            text=True,
            timeout=10,
        )
        assert insecure_cli.returncode != 0, insecure_cli
        assert "capability_file_insecure" in insecure_cli.stderr, insecure_cli.stderr
        cli_started = cli_json(
            args.cli,
            args.socket,
            "--capability-file",
            str(cli_capability_path),
            "agent",
            "exec",
            pane_id,
            "/bin/sh",
            "-c",
            "printf 'CLI_AGENT_OK'; exit 0",
        )
        cli_terminal = cli_json(
            args.cli,
            args.socket,
            "--capability-file",
            str(cli_capability_path),
            "agent",
            "wait",
            cli_started["command_id"],
            "--timeout-ms",
            "3000",
        )
        assert cli_terminal["wait_state"] == "done", cli_terminal
        cli_events = cli_json(
            args.cli,
            args.socket,
            "--capability-file",
            str(cli_capability_path),
            "agent",
            "events",
            cli_started["command_id"],
        )
        assert decode_stream(cli_events["events"], "stdout") == b"CLI_AGENT_OK"

        recovering = exec_command(
            args.socket,
            exec_capability,
            pane_id,
            "/bin/sh",
            "-c",
            "sleep 0.8; exit 0",
        )
        crash_qwed(qwed)
        previous_management = management
        qwed = start_qwed(
            args.qwed,
            args.socket,
            log_path,
            credentials_dir=credentials_dir,
        )
        _, management = bootstrap_management(args.socket)
        recovered = wait_result(
            args.socket,
            recovering["command_id"],
            exec_capability,
            5000,
        )
        assert recovered["wait_state"] == "done", recovered

        lost_session = request(
            args.socket,
            "SESSION_CREATE",
            {
                "session_id": "stage9-agent-lost",
                "program": "sleep",
                "args": ["120"],
                "owner": OWNER,
            },
        )
        lost_pane_id = lost_session["pane_id"]
        lost_exec_capability = issue_capability(
            args.socket,
            management,
            root_principal,
            "agent_exec",
            {"kind": "pane", "pane_id": lost_pane_id},
        )["token"]
        lost_command = exec_command(
            args.socket,
            lost_exec_capability,
            lost_pane_id,
            "/bin/sleep",
            "30",
        )
        lost_pane = request(args.socket, "PANE_STATUS", {"pane_id": lost_pane_id})
        os.kill(lost_pane["worker_pid"], signal.SIGKILL)
        lost = wait_for(
            lambda: lost_agent_status(
                args.socket,
                lost_command["command_id"],
                lost_exec_capability,
            ),
            "agent command did not become lost after worker loss",
            timeout=8,
        )
        assert lost["exit_code"] is None, lost

        capability_tokens = [
            previous_management,
            management,
            exec_capability,
            cross_pane_capability,
            expiring_capability,
            ancestor_capability,
            root_read,
            root_terminate,
            freeze_capability,
            frozen_read,
            low_trust_management,
        ]
        for suffix in ("", "-wal"):
            database = Path(f"{args.socket}.sqlite{suffix}")
            if not database.exists():
                continue
            contents = database.read_bytes()
            assert COMMAND_SECRET not in contents, (
                f"command credential leaked to {database}"
            )
            for token in capability_tokens:
                assert token.encode() not in contents, (
                    f"capability token leaked to {database}"
                )
        if log_path.exists():
            assert COMMAND_SECRET not in log_path.read_bytes(), (
                "command credential leaked to qwed or worker logs"
            )

        print(
            json.dumps(
                {
                    "case": (
                        "AGENT-001/AGENT-002/AGENT-003/"
                        "AGENT-004/AGENT-005/AGENT-006/"
                        "AGENT-007/AGENT-008/AGENT-009/"
                        "AGENT-010/AGENT-011/AGENT-012"
                    ),
                    "exec": "pass",
                    "split_streams": "pass",
                    "wait_timeout": "pass",
                    "signal": "pass",
                    "deadline": "pass",
                    "bounded_replay": "pass",
                    "pane_resource_scope": scope_check,
                    "cli": "pass",
                    "qwed_restart": "pass",
                    "lost_state": "pass",
                    "capability_scope": "pass",
                    "ancestor_revocation": "pass",
                    "administrative_revocation": "pass",
                    "freeze_escalation": "pass",
                    "low_trust_denial": "pass",
                    "capability_redaction": "pass",
                    "command_credential_descriptor": "pass",
                    "command_credential_redaction": "pass",
                    "credential_failure_closed": "pass",
                    "observer_push": "pass",
                    "observer_read_only": "pass",
                    "pty_merged_output": "pass",
                    "foreground_idle": "pass",
                },
                separators=(",", ":"),
            )
        )
    finally:
        stop_qwed(qwed, args.socket)
        for suffix in ("", "-shm", "-wal"):
            path = Path(f"{args.socket}{suffix}")
            if path.exists():
                path.unlink()
        if log_path.exists() and qwed is not None and qwed.returncode not in (None, 0):
            print(log_path.read_text(errors="replace")[-8000:], file=sys.stderr)


if __name__ == "__main__":
    main()
