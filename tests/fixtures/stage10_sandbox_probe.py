#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import socket
import sys
import tempfile
import threading
import time
from pathlib import Path

from stage7_resource_probe import (
    cgroup_path,
    claim,
    crash_qwed,
    process_gone,
    request,
    request_with_capability,
    start_qwed,
    stop_qwed,
    wait_for,
)


OWNER = "stage10-sandbox"
SESSION_ID = "stage10-sandbox-session"
COMMAND_SECRET = b"stage10-sandbox-credential-4d1e"
HOST_ENV_SECRET = "stage10-host-environment-secret-28c9"


def issue_capability(socket_path, management, principal_id, pane_id, profile=None):
    payload = {
        "principal_id": principal_id,
        "operation": "agent_exec",
        "scope": {"kind": "pane", "pane_id": pane_id},
        "ttl_ms": 60000,
    }
    if profile is not None:
        payload["sandbox_profile"] = profile
    return request_with_capability(
        socket_path,
        "SECURITY_CAPABILITY_ISSUE",
        payload,
        management,
    )


def exec_command(
    socket_path,
    capability,
    pane_id,
    program,
    *arguments,
    credential=None,
    pty=False,
    requested_profile=None,
):
    payload = {
        "pane_id": pane_id,
        "program": program,
        "args": list(arguments),
        "owner": OWNER,
    }
    if credential is not None:
        payload["credential"] = credential
    if pty:
        payload["pty"] = True
        payload["rows"] = 24
        payload["columns"] = 80
    if requested_profile is not None:
        payload["sandbox_profile"] = requested_profile
    return request_with_capability(socket_path, "AGENT_EXEC", payload, capability)


def wait_terminal(socket_path, capability, command_id, timeout_ms=5000):
    return request_with_capability(
        socket_path,
        "AGENT_COMMAND_WAIT_IDLE",
        {"command_id": command_id, "timeout_ms": timeout_ms},
        capability,
    )


def events(socket_path, capability, command_id):
    result = request_with_capability(
        socket_path,
        "AGENT_COMMAND_EVENTS",
        {"command_id": command_id, "cursor": 0, "limit": 512},
        capability,
    )
    assert result["replay_gap"] is None, result
    return result["events"]


def decode_stream(command_events, stream):
    output = []
    cursor = 0
    for event in command_events:
        if event["type"] != "output" or event["stream"] != stream:
            continue
        data = bytes.fromhex(event["data_hex"])
        assert event["cursor"] == cursor, event
        cursor += len(data)
        output.append(data)
    return b"".join(output)


def assert_done(socket_path, capability, started, expected_code=0):
    terminal = wait_terminal(socket_path, capability, started["command_id"])
    if (
        terminal["wait_state"] != "done"
        or terminal["exit_code"] != expected_code
    ):
        command_events = events(socket_path, capability, started["command_id"])
        raise AssertionError(
            {
                "terminal": terminal,
                "stdout": decode_stream(command_events, "stdout").decode(
                    errors="replace"
                ),
                "stderr": decode_stream(command_events, "stderr").decode(
                    errors="replace"
                ),
            }
        )
    return terminal


def started_sandbox(socket_path, capability, command_id):
    deadline = time.monotonic() + 8
    last_status = None
    while time.monotonic() < deadline:
        status = request_with_capability(
            socket_path,
            "AGENT_COMMAND_STATUS",
            {"command_id": command_id},
            capability,
        )
        last_status = status
        sandbox = status.get("sandbox")
        if status.get("process_identity") and sandbox and sandbox.get("child_pid"):
            return status
        if status.get("state") in ("done", "failed", "lost"):
            break
        time.sleep(0.02)
    command_events = events(socket_path, capability, command_id)
    raise RuntimeError(
        {
            "message": "sandboxed command did not expose startup identity",
            "status": last_status,
            "stdout": decode_stream(command_events, "stdout").decode(errors="replace"),
            "stderr": decode_stream(command_events, "stderr").decode(errors="replace"),
        }
    )


def profile(
    workspace,
    workspace_mode="read_only",
    protected_paths=None,
    network="isolated_loopback",
):
    return {
        "version": 1,
        "workspace": str(workspace),
        "workspace_mode": workspace_mode,
        "protected_paths": [str(path) for path in protected_paths or []],
        "network": network,
        "home": "hidden",
        "private_tmp": True,
        "process_namespace": "isolated",
        "seccomp": {"mode": "none"},
    }


def tcp_capture():
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(3)
    received = []

    def accept():
        try:
            connection, _ = listener.accept()
            with connection:
                connection.settimeout(2)
                received.append(connection.recv(16384))
        except OSError:
            pass

    worker = threading.Thread(target=accept, daemon=True)
    worker.start()
    return listener, worker, received, listener.getsockname()[1]


def run_positive_probe(
    socket_path,
    workspace,
):
    bootstrap_path = Path(f"{socket_path}.runtime/security.bootstrap")
    management = bootstrap_path.read_text().strip()
    bootstrap = request_with_capability(
        socket_path,
        "SECURITY_BOOTSTRAP",
        {},
        management,
    )
    principal_id = bootstrap["principal"]["principal_id"]
    created = request(
        socket_path,
        "SESSION_CREATE",
        {
            "session_id": SESSION_ID,
            "program": "/bin/sh",
            "args": ["-c", "exec sleep 120"],
            "owner": OWNER,
        },
    )
    assert created.get("running") is True, created
    pane_id = created["pane_id"]

    read_only_profile = profile(workspace)
    read_only = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        read_only_profile,
    )
    assert read_only.get("token"), read_only
    assert read_only["sandbox_profile"] == read_only_profile, read_only
    audit = read_only["sandbox_audit"]
    assert audit["backend_path"].endswith("bwrap"), audit
    assert audit["backend_version"].startswith("bubblewrap"), audit
    assert len(audit["profile_hash"]) == 64, audit
    assert audit["probe_time_ms"] > 0, audit
    read_only_capability = read_only["token"]

    basic = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sh",
        "-c",
        "printf 'SBX_OUT'; printf 'SBX_ERR' >&2; exit 7",
    )
    assert_done(socket_path, read_only_capability, basic, expected_code=7)
    basic_events = events(socket_path, read_only_capability, basic["command_id"])
    assert decode_stream(basic_events, "stdout") == b"SBX_OUT", basic_events
    assert decode_stream(basic_events, "stderr") == b"SBX_ERR", basic_events

    active = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sleep",
        "5",
    )
    active_status = started_sandbox(
        socket_path,
        read_only_capability,
        active["command_id"],
    )
    pane = request(socket_path, "PANE_STATUS", {"pane_id": pane_id})
    wrapper_pid = active_status["process_identity"]["pid"]
    child_pid = active_status["sandbox"]["child_pid"]
    assert wrapper_pid != child_pid, active_status
    assert active_status["sandbox"]["setuid_denied"] is True
    assert active_status["sandbox"]["no_new_privs"] is True
    assert cgroup_path(wrapper_pid) == cgroup_path(pane["pid"]), active_status
    assert cgroup_path(child_pid) == cgroup_path(pane["pid"]), active_status
    request_with_capability(
        socket_path,
        "AGENT_COMMAND_TERMINATE",
        {"command_id": active["command_id"]},
        read_only_capability,
    )
    terminated = wait_terminal(
        socket_path,
        read_only_capability,
        active["command_id"],
        3000,
    )
    assert terminated["wait_state"] == "failed", terminated

    sandbox_signal = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sleep",
        "30",
    )
    started_sandbox(
        socket_path,
        read_only_capability,
        sandbox_signal["command_id"],
    )
    request_with_capability(
        socket_path,
        "AGENT_COMMAND_SIGNAL",
        {"command_id": sandbox_signal["command_id"], "signal": "SIGTERM"},
        read_only_capability,
    )
    sandbox_signal_terminal = wait_terminal(
        socket_path,
        read_only_capability,
        sandbox_signal["command_id"],
        3000,
    )
    assert sandbox_signal_terminal["wait_state"] == "failed", (
        sandbox_signal_terminal
    )
    assert (
        sandbox_signal_terminal["exit_signal"] == "SIGTERM"
    ), sandbox_signal_terminal

    ro_target = workspace / "read-only-write.txt"
    ro_attempt = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            f"if printf denied > {ro_target}; then exit 97; "
            "else printf 'RO_DENIED'; fi"
        ),
    )
    assert_done(socket_path, read_only_capability, ro_attempt)
    assert not ro_target.exists(), ro_target
    ro_events = events(socket_path, read_only_capability, ro_attempt["command_id"])
    assert decode_stream(ro_events, "stdout") == b"RO_DENIED", ro_events
    override_attempt = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/true",
        requested_profile=profile(workspace, workspace_mode="read_write"),
    )
    assert (
        override_attempt["code"] == "sandbox_profile_not_client_selectable"
    ), override_attempt

    unsandboxed = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
    )
    assert unsandboxed["sandbox_profile"] is None, unsandboxed
    request_only_profile = exec_command(
        socket_path,
        unsandboxed["token"],
        pane_id,
        "/bin/true",
        requested_profile=read_only_profile,
    )
    assert (
        request_only_profile["code"] == "sandbox_profile_not_client_selectable"
    ), request_only_profile
    unsandboxed_exec = exec_command(
        socket_path,
        unsandboxed["token"],
        pane_id,
        "/bin/sh",
        "-c",
        f"printf 'UNSANDBOXED' > {ro_target}",
    )
    assert_done(socket_path, unsandboxed["token"], unsandboxed_exec)
    assert ro_target.read_text() == "UNSANDBOXED"
    ro_target.unlink()

    rw_target = workspace / "read-write-ok.txt"
    rw_profile = profile(workspace, workspace_mode="read_write")
    rw_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        rw_profile,
    )["token"]
    rw_command = exec_command(
        socket_path,
        rw_capability,
        pane_id,
        "/bin/sh",
        "-c",
        f"printf 'RW_OK' > {rw_target}",
    )
    assert_done(socket_path, rw_capability, rw_command)
    assert rw_target.read_text() == "RW_OK"

    protected_git = workspace / ".git"
    protected_target = protected_git / "sandbox-blocked"
    protected_profile = profile(
        workspace,
        workspace_mode="read_write",
        protected_paths=[protected_git],
    )
    protected_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        protected_profile,
    )["token"]
    protected_command = exec_command(
        socket_path,
        protected_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            f"if printf 'blocked' > {protected_target}; then exit 97; "
            "else printf 'PROTECTED'; fi"
        ),
    )
    assert_done(socket_path, protected_capability, protected_command)
    assert not protected_target.exists(), protected_target

    protected_link = workspace / "protected-link"
    protected_link.symlink_to(protected_git)
    linked_profile = profile(
        workspace,
        workspace_mode="read_write",
        protected_paths=[protected_link],
    )
    linked_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        linked_profile,
    )["token"]
    linked_target = protected_link / "symlink-blocked"
    linked_command = exec_command(
        socket_path,
        linked_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            f"if printf 'blocked' > {linked_target}; then exit 97; "
            "else printf 'SYMLINK_PROTECTED'; fi"
        ),
    )
    assert_done(socket_path, linked_capability, linked_command)
    assert not (protected_git / "symlink-blocked").exists()
    protected_link.unlink()

    home_profile = profile(workspace)
    home_profile["home"] = "read_only"
    home_marker = Path(os.environ["HOME"]) / "qwe-sandbox-write"
    if home_marker.exists():
        home_marker.unlink()
    home_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        home_profile,
    )["token"]
    home_command = exec_command(
        socket_path,
        home_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            "test -d \"$HOME\" && "
            "if printf denied > \"$HOME/qwe-sandbox-write\"; "
            "then exit 97; else printf 'HOME_READ_ONLY'; fi"
        ),
    )
    assert_done(socket_path, home_capability, home_command)
    home_events = events(socket_path, home_capability, home_command["command_id"])
    assert decode_stream(home_events, "stdout") == b"HOME_READ_ONLY"
    assert not home_marker.exists()

    shared_profile = profile(workspace)
    shared_profile["process_namespace"] = "shared"
    shared_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        shared_profile,
    )["token"]
    shared_command = exec_command(
        socket_path,
        shared_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            f"test -e /proc/{pane['worker_pid']} && "
            f"test -e /proc/{pane['pid']} && printf 'PID_NAMESPACE_SHARED'"
        ),
    )
    assert_done(socket_path, shared_capability, shared_command)
    shared_events = events(
        socket_path,
        shared_capability,
        shared_command["command_id"],
    )
    assert decode_stream(shared_events, "stdout") == b"PID_NAMESPACE_SHARED"

    inherited_profile = profile(workspace, network="inherit")
    inherited_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        inherited_profile,
    )["token"]
    inherited_listener, inherited_worker, inherited_data, inherited_port = tcp_capture()
    inherited_script = (
        "import os,socket,time;"
        "secret=os.read(3,16384);"
        f"s=socket.create_connection(('127.0.0.1',{inherited_port}),timeout=2);"
        "s.sendall(secret);s.close();"
        "print('NETWORK_INHERIT_CREDENTIAL_SENT');"
        "time.sleep(0.5)"
    )
    inherited_command = exec_command(
        socket_path,
        inherited_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        inherited_script,
        credential="CMD_KEY",
    )
    inherited_status = started_sandbox(
        socket_path,
        inherited_capability,
        inherited_command["command_id"],
    )
    risk = inherited_status["sandbox"]["risk"]
    assert risk["network_inherit"] is True, risk
    assert risk["credential_exposed"] is True, risk
    assert risk["credential_network_exfiltration"] is True, risk
    assert_done(socket_path, inherited_capability, inherited_command)
    inherited_events = events(
        socket_path,
        inherited_capability,
        inherited_command["command_id"],
    )
    assert any(
        event["type"] == "sandbox_risk_observed"
        and event["network_inherit"] is True
        and event["credential_exposed"] is True
        for event in inherited_events
    ), inherited_events
    inherited_worker.join(4)
    inherited_listener.close()
    assert inherited_data and inherited_data[0] == COMMAND_SECRET, inherited_data

    isolated_listener, isolated_worker, isolated_data, isolated_port = tcp_capture()
    isolated_exfil_script = (
        "import os,socket,sys\n"
        "secret=os.read(3,16384)\n"
        "try:\n"
        f"    s=socket.create_connection(('127.0.0.1',{isolated_port}),timeout=1)\n"
        "except OSError:\n"
        "    print('ISOLATED_EXFIL_DENIED')\n"
        "    sys.exit(0)\n"
        "s.sendall(secret)\n"
        "s.close()\n"
        "print('ISOLATED_EXFIL_LEAKED')\n"
        "sys.exit(97)\n"
    )
    isolated_exfil = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        isolated_exfil_script,
        credential="CMD_KEY",
    )
    assert_done(socket_path, read_only_capability, isolated_exfil)
    isolated_worker.join(4)
    isolated_listener.close()
    assert not isolated_data, isolated_data

    host_tmp = Path(f"/tmp/qwe-stage10-host-{os.getpid()}")
    host_tmp.write_text("host-only")
    outside_host = Path(socket_path).parent / "outside-sandbox-host.txt"
    outside_host.write_text("outside-marker")
    outside_link = workspace / "outside-link"
    outside_link.symlink_to(outside_host)
    hidden = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            f"test ! -e \"$HOME\" && test ! -e {host_tmp} && "
            f"test ! -e {outside_link} && "
            'test -z "${QWE_SANDBOX_HOST_SECRET+x}" && '
            f"test ! -e /proc/{pane['worker_pid']} && "
            f"test ! -e /proc/{pane['pid']} && "
            "test ! -e /etc/shadow && test ! -e /sys/kernel && "
            "printf 'HOME_HIDDEN TMP_PRIVATE ENV_CLEAR PID_NS FS_BOUNDED'"
        ),
    )
    assert_done(socket_path, read_only_capability, hidden)
    hidden_events = events(socket_path, read_only_capability, hidden["command_id"])
    assert (
        decode_stream(hidden_events, "stdout")
        == b"HOME_HIDDEN TMP_PRIVATE ENV_CLEAR PID_NS FS_BOUNDED"
    )
    host_tmp.unlink()
    assert outside_host.read_text() == "outside-marker"
    outside_link.unlink()
    outside_host.unlink()

    network_script = (
        "import fcntl,socket,struct;"
        "from pathlib import Path;"
        "s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);"
        "ifr=struct.pack('256s',b'lo');"
        "flags=struct.unpack('H',fcntl.ioctl(s,0x8913,ifr)[16:18])[0];"
        "assert flags & 1, flags;"
        "assert flags & 8, flags;"
        "routes=Path('/proc/net/route').read_text().splitlines()[1:];"
        "defaults=[line for line in routes if line.split()[1] == '00000000'];"
        "assert not defaults, defaults;"
        "server=socket.socket();"
        "server.bind(('127.0.0.1',0));"
        "server.listen(1);"
        "client=socket.socket();"
        "client.settimeout(2);"
        "client.connect(server.getsockname());"
        "server.accept();"
        "print('LOOPBACK_UP NO_DEFAULT_ROUTE LOOPBACK_CONNECT')"
    )
    isolated = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        network_script,
    )
    assert_done(socket_path, read_only_capability, isolated)
    isolated_events = events(
        socket_path,
        read_only_capability,
        isolated["command_id"],
    )
    isolated_stdout = decode_stream(isolated_events, "stdout").strip()
    assert (
        isolated_stdout
        == b"LOOPBACK_UP NO_DEFAULT_ROUTE LOOPBACK_CONNECT"
    ), isolated_stdout

    privilege_boundary = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sh",
        "-c",
        (
            "grep -Eq '^NoNewPrivs:[[:space:]]+1$' /proc/self/status && "
            "printf 'NO_NEW_PRIVS_SETUID_DENIED'"
        ),
    )
    assert_done(socket_path, read_only_capability, privilege_boundary)
    privilege_events = events(
        socket_path,
        read_only_capability,
        privilege_boundary["command_id"],
    )
    assert (
        decode_stream(privilege_events, "stdout")
        == b"NO_NEW_PRIVS_SETUID_DENIED"
    )

    credential_script = (
        "import os,time;"
        "secret=os.read(3,16384);"
        "os.write(1,b'CREDENTIAL:'+secret+b'\\n');"
        "time.sleep(0.5)"
    )
    credentialed = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        credential_script,
        credential="CMD_KEY",
    )
    credentialed_status = started_sandbox(
        socket_path,
        read_only_capability,
        credentialed["command_id"],
    )
    child_pid = credentialed_status["sandbox"]["child_pid"]
    environment = Path(f"/proc/{child_pid}/environ").read_bytes()
    assert b"QWE_CREDENTIAL_FD=3" in environment, environment
    assert COMMAND_SECRET not in environment, environment
    assert_done(socket_path, read_only_capability, credentialed)
    credential_events = events(
        socket_path,
        read_only_capability,
        credentialed["command_id"],
    )
    assert decode_stream(credential_events, "stdout") == b"CREDENTIAL:[REDACTED]\n"
    assert any(
        event["type"] == "redaction_applied"
        and event["policy"] == "credential_exact"
        for event in credential_events
    ), credential_events

    pty_tiocsti_script = (
        "import ctypes,os,sys,time;"
        "secret=os.read(3,16384);"
        "print('PTY_CREDENTIAL='+secret.decode(),flush=True);"
        "libc=ctypes.CDLL(None,use_errno=True);"
        "value=ctypes.create_string_buffer(b'X');"
        "result=libc.ioctl(0,0x5412,value);"
        "error=ctypes.get_errno();"
        "print(f'TIOCSTI_RESULT={result}:{error}',flush=True);"
        "time.sleep(0.5);"
        "sys.exit(0 if result == -1 and error == 1 else 97)"
    )
    pty_command = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        pty_tiocsti_script,
        credential="CMD_KEY",
        pty=True,
    )
    pty_status = started_sandbox(
        socket_path,
        read_only_capability,
        pty_command["command_id"],
    )
    assert (
        pty_status["process_identity"]["pid"]
        == pty_status["sandbox"]["wrapper_pid"]
    )
    assert (
        pty_status["sandbox"]["signal_pgid"]
        == pty_status["sandbox"]["child_pid"]
    ), pty_status
    pty_pane = request(socket_path, "PANE_STATUS", {"pane_id": pane_id})
    assert (
        cgroup_path(pty_status["sandbox"]["wrapper_pid"])
        == cgroup_path(pty_pane["pid"])
    )
    assert (
        cgroup_path(pty_status["sandbox"]["child_pid"])
        == cgroup_path(pty_pane["pid"])
    )
    assert_done(socket_path, read_only_capability, pty_command)
    pty_events = events(socket_path, read_only_capability, pty_command["command_id"])
    pty_output = decode_stream(pty_events, "pty")
    assert b"TIOCSTI_RESULT=-1:1" in pty_output, pty_output
    assert b"PTY_CREDENTIAL=[REDACTED]" in pty_output, pty_output
    assert any(
        event["type"] == "redaction_applied"
        and event["policy"] == "credential_exact"
        for event in pty_events
    ), pty_events

    pty_size = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sh",
        "-c",
        "stty size",
        pty=True,
    )
    assert_done(socket_path, read_only_capability, pty_size)
    pty_size_events = events(
        socket_path,
        read_only_capability,
        pty_size["command_id"],
    )
    assert decode_stream(pty_size_events, "pty").strip() == b"24 80"

    pty_idle = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/bin/sh",
        pty=True,
    )
    idle = wait_terminal(
        socket_path,
        read_only_capability,
        pty_idle["command_id"],
        3000,
    )
    assert idle["wait_state"] == "idle", idle
    idle_status = request_with_capability(
        socket_path,
        "AGENT_COMMAND_STATUS",
        {"command_id": pty_idle["command_id"]},
        read_only_capability,
    )
    idle_wrapper = idle_status["sandbox"]["wrapper_pid"]
    idle_child = idle_status["sandbox"]["child_pid"]
    interrupted = request_with_capability(
        socket_path,
        "AGENT_COMMAND_SIGNAL",
        {"command_id": pty_idle["command_id"], "signal": "SIGINT"},
        read_only_capability,
    )
    assert interrupted["state"] in ("idle", "running"), interrupted
    time.sleep(0.1)
    interrupted_status = request_with_capability(
        socket_path,
        "AGENT_COMMAND_STATUS",
        {"command_id": pty_idle["command_id"]},
        read_only_capability,
    )
    assert interrupted_status["state"] in ("idle", "running"), interrupted_status
    interrupted_events = events(
        socket_path,
        read_only_capability,
        pty_idle["command_id"],
    )
    assert any(
        event["type"] == "signal_delivered" and event["signal"] == "SIGINT"
        for event in interrupted_events
    ), interrupted_events
    request_with_capability(
        socket_path,
        "AGENT_COMMAND_TERMINATE",
        {"command_id": pty_idle["command_id"]},
        read_only_capability,
    )
    idle_terminal = wait_terminal(
        socket_path,
        read_only_capability,
        pty_idle["command_id"],
        3000,
    )
    assert idle_terminal["wait_state"] == "failed", {
        "terminal": idle_terminal,
        "status": idle_status,
    }
    wait_for(
        lambda: process_gone(idle_wrapper),
        "sandboxed PTY wrapper survived termination",
    )
    wait_for(
        lambda: process_gone(idle_child),
        "sandboxed PTY child survived termination",
    )

    capability_tokens = [
        management,
        read_only_capability,
        unsandboxed["token"],
        rw_capability,
        protected_capability,
        linked_capability,
    ]
    for suffix in ("", "-wal"):
        database = Path(f"{socket_path}.sqlite{suffix}")
        if not database.exists():
            continue
        contents = database.read_bytes()
        assert COMMAND_SECRET not in contents
        assert HOST_ENV_SECRET.encode() not in contents
        for token in capability_tokens:
            assert token.encode() not in contents
    log_path = Path(f"{socket_path}.runtime").parent / "qwed-stage10.log"
    if log_path.exists():
        contents = log_path.read_bytes()
        assert COMMAND_SECRET not in contents
        assert HOST_ENV_SECRET.encode() not in contents

    return {
        "profile_authorization": "pass",
        "request_cannot_enable": "pass",
        "request_cannot_downgrade": "pass",
        "workspace_ro_rw": "pass",
        "protected_path": "pass",
        "symlink_protection": "pass",
        "home_read_only": "pass",
        "process_namespace_shared": "pass",
        "network_inherit": "pass",
        "credential_network_exfil_confirmed": "pass",
        "sandbox_risk_audit": "pass",
        "sandbox_audit_persistence": "pass",
        "sandbox_risk_persistence": "pass",
        "home_and_tmp": "pass",
        "environment_and_pid_isolation": "pass",
        "filesystem_boundary": "pass",
        "network_isolation": "pass",
        "loopback": "pass",
        "setuid_no_new_privs": "pass",
        "fd3_and_redaction": "pass",
        "cgroup": "pass",
        "pty_sandbox": "pass",
        "pty_sigint": "pass",
        "sandbox_signal": "pass",
    }, pane_id, read_only_capability


def lost_agent_status(socket_path, capability, command_id):
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


def wait_cgroup_drained(cgroup, timeout=8):
    def drained():
        procs = cgroup / "cgroup.procs"
        if not procs.exists():
            return True
        return not procs.read_text().strip()

    return wait_for(drained, f"sandbox cgroup {cgroup} retained live processes", timeout)


def run_recovery_probe(
    qwed_binary,
    socket_path,
    log_path,
    qwed,
    workspace,
    pane_id,
    capability,
):
    recovering = exec_command(
        socket_path,
        capability,
        pane_id,
        "/bin/sh",
        "-c",
        "sleep 0.8; printf 'RECOVERED_AFTER_QWED_RESTART'",
    )
    before_restart = started_sandbox(
        socket_path,
        capability,
        recovering["command_id"],
    )
    crash_qwed(qwed)
    qwed = start_qwed(qwed_binary, socket_path, log_path)
    after_restart = started_sandbox(
        socket_path,
        capability,
        recovering["command_id"],
    )
    assert (
        after_restart["sandbox"]["profile"]
        == before_restart["sandbox"]["profile"]
    ), after_restart
    assert after_restart["process_identity"] == before_restart["process_identity"]
    assert (
        after_restart["sandbox"]["child_pid"]
        == before_restart["sandbox"]["child_pid"]
    )
    assert (
        after_restart["sandbox"]["audit"]
        == before_restart["sandbox"]["audit"]
    )
    recovered = assert_done(socket_path, capability, recovering)
    assert recovered["exit_code"] == 0, recovered
    recovered_events = events(socket_path, capability, recovering["command_id"])
    assert (
        decode_stream(recovered_events, "stdout")
        == b"RECOVERED_AFTER_QWED_RESTART"
    ), recovered_events

    teardown = exec_command(
        socket_path,
        capability,
        pane_id,
        "/bin/sh",
        "-c",
        "sleep 120 & wait",
    )
    teardown_status = started_sandbox(
        socket_path,
        capability,
        teardown["command_id"],
    )
    teardown_wrapper = teardown_status["process_identity"]["pid"]
    teardown_child = teardown_status["sandbox"]["child_pid"]
    teardown_cgroup = cgroup_path(teardown_child)
    pane = request(socket_path, "PANE_STATUS", {"pane_id": pane_id})
    generation = claim(socket_path, SESSION_ID, OWNER)
    closed = request(
        socket_path,
        "SESSION_KILL",
        {
            "session_id": SESSION_ID,
            "owner": OWNER,
            "generation": generation,
        },
    )
    assert closed.get("state") == "closed", closed
    wait_for(
        lambda: process_gone(pane["worker_pid"]),
        "Pane worker survived forced Session teardown",
    )
    wait_for(
        lambda: process_gone(teardown_wrapper),
        "Bubblewrap wrapper survived forced Session teardown",
    )
    wait_for(
        lambda: process_gone(teardown_child),
        "sandbox child survived forced Session teardown",
    )
    wait_cgroup_drained(teardown_cgroup)

    loss_session_id = "stage10-sandbox-worker-loss"
    loss_session = request(
        socket_path,
        "SESSION_CREATE",
        {
            "session_id": loss_session_id,
            "program": "/bin/sh",
            "args": ["-c", "exec /bin/sleep 120"],
            "owner": OWNER,
        },
    )
    assert loss_session.get("running") is True, loss_session
    loss_pane = loss_session["pane_id"]
    management = Path(
        f"{socket_path}.runtime/security.bootstrap"
    ).read_text().strip()
    principal_id = request_with_capability(
        socket_path,
        "SECURITY_BOOTSTRAP",
        {},
        management,
    )["principal"]["principal_id"]
    loss_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        loss_pane,
        profile(workspace),
    )["token"]
    loss_command = exec_command(
        socket_path,
        loss_capability,
        loss_pane,
        "/bin/sh",
        "-c",
        "sleep 120 & wait",
    )
    loss_status = started_sandbox(
        socket_path,
        loss_capability,
        loss_command["command_id"],
    )
    loss_wrapper = loss_status["process_identity"]["pid"]
    loss_child = loss_status["sandbox"]["child_pid"]
    loss_cgroup = cgroup_path(loss_child)
    loss_pane_status = request(
        socket_path,
        "PANE_STATUS",
        {"pane_id": loss_pane},
    )
    os.kill(loss_pane_status["worker_pid"], 9)
    lost = wait_for(
        lambda: lost_agent_status(
            socket_path,
            loss_capability,
            loss_command["command_id"],
        ),
        "sandboxed command did not become lost after worker loss",
        timeout=8,
    )
    assert lost["sandbox"]["enforced"] is True, lost
    assert lost["sandbox"]["risk"]["network_inherit"] is False, lost
    assert lost["sandbox"]["audit"]["profile_hash"], lost
    wait_for(
        lambda: process_gone(loss_wrapper),
        "Bubblewrap wrapper survived worker loss",
    )
    wait_for(
        lambda: process_gone(loss_child),
        "sandbox child survived worker loss",
    )
    wait_cgroup_drained(loss_cgroup)
    return qwed, {
        "qwed_restart": "pass",
        "pane_teardown": "pass",
        "worker_loss": "pass",
    }


def run_backend_unavailable_probe(qwed, root):
    socket_path = str(root / "qwed-no-bwrap.sock")
    log_path = root / "qwed-no-bwrap.log"
    empty_path = root / "empty-path"
    empty_path.mkdir()
    env = os.environ.copy()
    env["PATH"] = str(empty_path)
    env.pop("QWE_BWRAP_PATH", None)
    process = start_qwed(qwed, socket_path, log_path, env=env)
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
                "session_id": "stage10-no-bwrap",
                "program": "/bin/sh",
                "args": ["-c", "exec /bin/sleep 120"],
                "owner": OWNER,
            },
        )
        assert created.get("running") is True, created
        low_principal = request_with_capability(
            socket_path,
            "SECURITY_PRINCIPAL_CREATE",
            {"parent_id": principal_id},
            management,
        )["principal_id"]
        low_management = request_with_capability(
            socket_path,
            "SECURITY_CAPABILITY_ISSUE",
            {
                "principal_id": low_principal,
                "operation": "manage_principals",
                "scope": {"kind": "global"},
                "ttl_ms": 60000,
            },
            management,
        )["token"]
        result = issue_capability(
            socket_path,
            management,
            principal_id,
            created["pane_id"],
            profile(root),
        )
        assert result.get("code") == "sandbox_backend_unavailable", result
        assert result["details"]["reason"] == "bwrap_missing", result
        assert result["details"]["diagnostics"]["environment"] in ("host", "container")
        assert "stderr" in result["details"]["diagnostics"], result
        assert "backend_path" in result["details"]["diagnostics"], result
        low_result = issue_capability(
            socket_path,
            low_management,
            low_principal,
            created["pane_id"],
            profile(root),
        )
        assert low_result.get("code") == "sandbox_backend_unavailable", low_result
        assert low_result["details"]["reason"] == "bwrap_missing", low_result
        low_diagnostics = low_result["details"]["diagnostics"]
        assert low_diagnostics["environment"] in ("host", "container"), low_result
        for restricted in (
            "stderr",
            "sysctls",
            "backend_path",
            "backend_version",
            "probe_time_ms",
        ):
            assert restricted not in low_diagnostics, low_result
        (empty_path / "bwrap").symlink_to(shutil.which("bwrap"))
        recovered = issue_capability(
            socket_path,
            management,
            low_principal,
            created["pane_id"],
            profile(root),
        )
        assert recovered.get("token"), {
            "message": "fresh capability probe did not observe the repaired backend",
            "result": recovered,
        }
        (empty_path / "bwrap").unlink()
        failed_exec = exec_command(
            socket_path,
            recovered["token"],
            created["pane_id"],
            "/bin/true",
        )
        failed_terminal = wait_terminal(
            socket_path,
            recovered["token"],
            failed_exec["command_id"],
            3000,
        )
        assert failed_terminal["wait_state"] == "failed", failed_terminal
        assert (
            failed_terminal.get("error_details", {}).get("reason")
            == "bwrap_missing"
        ), failed_terminal
        restricted_diagnostics = failed_terminal["error_details"]["diagnostics"]
        assert "stderr" not in restricted_diagnostics, failed_terminal
        assert "backend_path" not in restricted_diagnostics, failed_terminal
    finally:
        stop_qwed(process, socket_path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    if not sys.platform.startswith("linux") or shutil.which("bwrap") is None:
        print(
            json.dumps(
                {
                    "case": "SANDBOX-004/SANDBOX-005/SANDBOX-006/SANDBOX-007",
                    "status": "skip",
                    "reason": "Linux and Bubblewrap are required",
                }
            )
        )
        return

    root = Path(args.socket).parent
    workspace = Path(tempfile.mkdtemp(prefix="qwe-stage10-workspace-", dir=root))
    (workspace / ".git").mkdir()
    credentials_dir = root / "credentials"
    credentials_dir.mkdir(mode=0o700)
    credential_path = credentials_dir / "CMD_KEY"
    credential_path.write_bytes(COMMAND_SECRET)
    credential_path.chmod(0o600)
    qwed = None
    try:
        qwed_env = os.environ.copy()
        qwed_env["QWE_SANDBOX_HOST_SECRET"] = HOST_ENV_SECRET
        qwed_env["QWE_BWRAP_PATH"] = shutil.which("bwrap")
        qwed = start_qwed(
            args.qwed,
            args.socket,
            root / "qwed-stage10.log",
            credentials_dir=credentials_dir,
            env=qwed_env,
        )
        results = run_positive_probe(
            args.socket,
            workspace,
        )
        results, pane_id, capability = results
        qwed, recovery_results = run_recovery_probe(
            args.qwed,
            args.socket,
            root / "qwed-stage10.log",
            qwed,
            workspace,
            pane_id,
            capability,
        )
        results.update(recovery_results)
        stop_qwed(qwed, args.socket)
        qwed = None
        run_backend_unavailable_probe(args.qwed, root)
        results["backend_unavailable"] = "pass"
        results["backend_probe_refresh"] = "pass"
        results["runtime_start_failure_details"] = "pass"
        results["diagnostics_visibility"] = "pass"
        print(
            json.dumps(
                {
                    "case": "SANDBOX-004/SANDBOX-005/SANDBOX-006/SANDBOX-007",
                    "status": "pass",
                    **results,
                }
            )
        )
    finally:
        if qwed is not None:
            stop_qwed(qwed, args.socket)


if __name__ == "__main__":
    main()
