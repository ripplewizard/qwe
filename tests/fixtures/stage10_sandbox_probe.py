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


def landlock_available():
    lsm = Path("/sys/kernel/security/lsm")
    return lsm.exists() and "landlock" in lsm.read_text().split(",")


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
    landlock=None,
):
    value = {
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
    value["landlock"] = landlock or {"mode": "best_effort"}
    return value


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


def unix_capture():
    name = f"\0qwe-stage10-{os.getpid()}-{time.time_ns()}".encode()
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(name)
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
    return listener, worker, received, name


def fifo_capture(path):
    os.mkfifo(path)
    received = []

    def receive():
        try:
            with open(path, "rb", buffering=0) as stream:
                received.append(stream.read(16384))
        except OSError:
            pass

    worker = threading.Thread(target=receive, daemon=True)
    worker.start()
    return worker, received


def run_positive_probe(
    socket_path,
    workspace,
    sandbox_init,
):
    runtime_init = workspace / "qwe-sandbox-init"
    shutil.copy2(sandbox_init, runtime_init)
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

    low_pane_principal = request_with_capability(
        socket_path,
        "SECURITY_PRINCIPAL_CREATE",
        {"parent_id": principal_id},
        management,
    )["principal_id"]
    low_pane_management = request_with_capability(
        socket_path,
        "SECURITY_CAPABILITY_ISSUE",
        {
            "principal_id": low_pane_principal,
            "operation": "manage_principals",
            "scope": {"kind": "global"},
            "ttl_ms": 60000,
        },
        management,
    )["token"]
    low_pane_input = request_with_capability(
        socket_path,
        "SECURITY_CAPABILITY_ISSUE",
        {
            "principal_id": low_pane_principal,
            "operation": "pane_input",
            "scope": {"kind": "pane", "pane_id": pane_id},
            "ttl_ms": 60000,
        },
        low_pane_management,
    )
    assert low_pane_input.get("code") == "insufficient_trust", low_pane_input

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
    assert len(audit["seccomp_profile_hash"]) == 64, audit
    assert isinstance(audit.get("fd_bind"), bool), audit
    landlock_expected = landlock_available()
    if landlock_expected:
        assert audit.get("landlock_abi", 0) >= 1, audit
    else:
        assert audit.get("landlock_abi") is None, audit
    read_only_capability = read_only["token"]

    sandboxed_pane = request_with_capability(
        socket_path,
        "SECURITY_CAPABILITY_ISSUE",
        {
            "principal_id": principal_id,
            "operation": "sandboxed_pane",
            "scope": {"kind": "session", "session_id": SESSION_ID},
            "sandbox_profile": read_only_profile,
            "ttl_ms": 60000,
        },
        management,
    )
    sandboxed_pane_capability = None
    if audit["fd_bind"]:
        assert sandboxed_pane.get("token"), sandboxed_pane
        assert sandboxed_pane["sandbox_audit"]["fd_bind"] is True, sandboxed_pane
        sandboxed_pane_capability = sandboxed_pane["token"]
    else:
        assert sandboxed_pane.get("code") == "sandbox_backend_unavailable", sandboxed_pane
        assert sandboxed_pane["details"]["reason"] == "fd_bind_unavailable", sandboxed_pane

    if sandboxed_pane_capability is not None:
        sandboxed_session_id = "stage10-sandboxed-pane"
        sandboxed_session_capability = request_with_capability(
            socket_path,
            "SECURITY_CAPABILITY_ISSUE",
            {
                "principal_id": principal_id,
                "operation": "sandboxed_pane",
                "scope": {"kind": "session", "session_id": sandboxed_session_id},
                "sandbox_profile": read_only_profile,
                "ttl_ms": 60000,
            },
            management,
        )["token"]
        missing_capability = request(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": sandboxed_session_id,
                "program": "/bin/bash",
                "sandboxed": True,
            },
        )
        assert missing_capability.get("code") == "capability_missing", missing_capability
        wrong_operation = request_with_capability(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": sandboxed_session_id,
                "program": "/bin/bash",
                "sandboxed": True,
            },
            management,
        )
        assert wrong_operation.get("code") == "capability_scope_denied", wrong_operation
        client_profile = request_with_capability(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": sandboxed_session_id,
                "program": "/bin/bash",
                "sandboxed": True,
                "sandbox_profile": read_only_profile,
            },
            sandboxed_session_capability,
        )
        assert (
            client_profile.get("code") == "sandbox_profile_not_client_selectable"
        ), client_profile
        credential_rejected = request_with_capability(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": sandboxed_session_id,
                "program": "/bin/bash",
                "sandboxed": True,
                "credential": "must-not-be-read",
            },
            sandboxed_session_capability,
        )
        assert credential_rejected.get("code") == "invalid_request", credential_rejected
        downgrade_rejected = request_with_capability(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": sandboxed_session_id,
                "program": "/bin/bash",
            },
            sandboxed_session_capability,
        )
        assert (
            downgrade_rejected.get("code") == "sandbox_capability_requires_sandboxed_pane"
        ), downgrade_rejected
        sandboxed_session = request_with_capability(
            socket_path,
            "SESSION_CREATE",
            {
                "session_id": sandboxed_session_id,
                "program": "/bin/sh",
                "args": [
                    "-c",
                    "printf 'SAFE_BEFORE\\033]0,TITLE_SHOULD_DROP\\007"
                    "\\033]52;c,Q0xJUA==\\007SAFE_AFTER\\n'; "
                    "exec /bin/bash --noprofile --norc -i",
                ],
                "owner": OWNER,
                "sandboxed": True,
            },
            sandboxed_session_capability,
        )
        assert sandboxed_session.get("running") is True, sandboxed_session
        assert sandboxed_session["sandbox"]["enforced"] is True, sandboxed_session
        assert sandboxed_session["sandbox"]["output_policy"] == "restricted", sandboxed_session
        assert (
            sandboxed_session["sandbox"]["input_policy"] == "disabled_pending_audit"
        ), sandboxed_session
        assert sandboxed_session["sandbox"]["profile"] == read_only_profile, sandboxed_session
        assert sandboxed_session["sandbox"]["audit"]["fd_bind"] is True, sandboxed_session
        sandboxed_pane_id = sandboxed_session["pane_id"]
        sandboxed_generation = claim(socket_path, sandboxed_session_id, OWNER)
        pane_downgrade_rejected = request_with_capability(
            socket_path,
            "PANE_CREATE",
            {
                "session_id": sandboxed_session_id,
                "owner": OWNER,
                "generation": sandboxed_generation,
                "program": "/bin/bash",
            },
            sandboxed_session_capability,
        )
        assert (
            pane_downgrade_rejected.get("code")
            == "sandbox_capability_requires_sandboxed_pane"
        ), pane_downgrade_rejected
        second_sandboxed_pane = request_with_capability(
            socket_path,
            "PANE_CREATE",
            {
                "session_id": sandboxed_session_id,
                "owner": OWNER,
                "generation": sandboxed_generation,
                "program": "/bin/bash",
                "args": ["--noprofile", "--norc", "-i"],
                "sandboxed": True,
            },
            sandboxed_session_capability,
        )
        assert second_sandboxed_pane.get("running") is True, second_sandboxed_pane
        assert second_sandboxed_pane["sandbox"]["enforced"] is True, second_sandboxed_pane
        sandboxed_pane_status = request(
            socket_path,
            "PANE_STATUS",
            {"pane_id": sandboxed_pane_id},
        )
        assert sandboxed_pane_status["sandbox"]["enforced"] is True, sandboxed_pane_status
        assert (
            sandboxed_pane_status["sandbox"]["audit"]
            == sandboxed_session["sandbox"]["audit"]
        ), sandboxed_pane_status
        sandboxed_pane_input = request_with_capability(
            socket_path,
            "SECURITY_CAPABILITY_ISSUE",
            {
                "principal_id": low_pane_principal,
                "operation": "pane_input",
                "scope": {"kind": "pane", "pane_id": sandboxed_pane_id},
                "ttl_ms": 60000,
            },
            low_pane_management,
        )
        assert sandboxed_pane_input.get("token"), sandboxed_pane_input
        attach_status = request(
            socket_path,
            "PANE_ATTACH",
            {"pane_id": sandboxed_pane_id},
        )
        assert attach_status.get("running") is True, attach_status
        assert "SAFE_BEFORE" in attach_status["screen"], attach_status
        assert "SAFE_AFTER" in attach_status["screen"], attach_status
        assert "TITLE_SHOULD_DROP" not in attach_status["screen"], attach_status
        journal = bytes.fromhex(attach_status["journal_hex"])
        assert b"TITLE_SHOULD_DROP" not in journal, journal
        assert b"Q0xJUA==" not in journal, journal
        input_blocked = request(
            socket_path,
            "SESSION_INPUT",
            {"session_id": sandboxed_session_id, "data": "true\n"},
        )
        assert input_blocked.get("code") == "sandbox_input_audit_unavailable", input_blocked

    landlock_off_profile = profile(workspace, landlock={"mode": "off"})
    landlock_off = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        landlock_off_profile,
    )
    assert landlock_off["sandbox_audit"].get("landlock_abi") is None, landlock_off
    landlock_off_command = exec_command(
        socket_path,
        landlock_off["token"],
        pane_id,
        "/bin/sh",
        "-c",
        "printf '%s' \"${QWE_LANDLOCK_ABI:-missing}\"",
    )
    assert_done(socket_path, landlock_off["token"], landlock_off_command)
    landlock_off_events = events(
        socket_path,
        landlock_off["token"],
        landlock_off_command["command_id"],
    )
    assert decode_stream(landlock_off_events, "stdout") == b"missing"

    if landlock_expected:
        required_landlock = issue_capability(
            socket_path,
            management,
            principal_id,
            pane_id,
            profile(workspace, landlock={"mode": "required", "min_abi": 1}),
        )
        assert required_landlock["sandbox_audit"]["landlock_abi"] >= 1, required_landlock

    landlock_required_failure = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        profile(workspace, landlock={"mode": "required", "min_abi": 999}),
    )
    assert (
        landlock_required_failure.get("code") == "sandbox_backend_unavailable"
    ), landlock_required_failure
    assert (
        landlock_required_failure["details"]["reason"]
        == "landlock_required_unavailable"
    ), landlock_required_failure

    strict_profile = profile(workspace)
    strict_profile["seccomp"] = {"mode": "named", "profile": "strict"}
    strict_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        strict_profile,
    )
    assert strict_capability.get("token"), strict_capability
    strict_hash = strict_capability["sandbox_audit"]["seccomp_profile_hash"]
    assert len(strict_hash) == 64, strict_capability
    assert strict_hash != audit["seccomp_profile_hash"], strict_capability
    strict_script = (
        "import ctypes,errno,os;"
        "machine=os.uname().machine;"
        "numbers={'x86_64':{'ptrace':101,'add_key':248,'request_key':249,"
        "'keyctl':250,'bpf':321,'userfaultfd':323,'io_uring':425},"
        "'aarch64':{'ptrace':117,'add_key':217,'request_key':218,"
        "'keyctl':219,'bpf':280,'userfaultfd':282,'io_uring':425}}[machine];"
        "libc=ctypes.CDLL(None,use_errno=True);"
        "CALL=lambda nr,*args:(ctypes.set_errno(0),libc.syscall(ctypes.c_long(nr),"
        "*[ctypes.c_long(a) for a in args]),ctypes.get_errno())[1:];"
        "assert CALL(numbers['ptrace'],0,0,0,0)[1] == errno.EPERM;"
        "assert CALL(numbers['add_key'],0,0,0,0,0)[1] == errno.EPERM;"
        "assert CALL(numbers['request_key'],0,0,0,0)[1] == errno.EPERM;"
        "assert CALL(numbers['keyctl'],0,0,0,0,0)[1] == errno.EPERM;"
        "assert CALL(numbers['bpf'],0,0,0)[1] == errno.EPERM;"
        "assert CALL(numbers['userfaultfd'],0)[1] == errno.EPERM;"
        "assert CALL(numbers['io_uring'],0,0)[1] == errno.ENOSYS;"
        "print('STRICT_SECCOMP_OK')"
    )
    strict_command = exec_command(
        socket_path,
        strict_capability["token"],
        pane_id,
        "/usr/bin/python3",
        "-c",
        strict_script,
    )
    assert_done(socket_path, strict_capability["token"], strict_command)
    strict_events = events(
        socket_path,
        strict_capability["token"],
        strict_command["command_id"],
    )
    assert decode_stream(strict_events, "stdout") == b"STRICT_SECCOMP_OK\n"
    strict_pty = exec_command(
        socket_path,
        strict_capability["token"],
        pane_id,
        "/bin/sh",
        "-c",
        "printf 'STRICT_PTY_OK'",
        pty=True,
    )
    assert_done(socket_path, strict_capability["token"], strict_pty)
    strict_pty_events = events(
        socket_path,
        strict_capability["token"],
        strict_pty["command_id"],
    )
    assert b"STRICT_PTY_OK" in decode_stream(strict_pty_events, "pty")

    nested_profile = profile(workspace, landlock={"mode": "off"})
    nested_profile["seccomp"] = {"mode": "named", "profile": "strict"}
    nested_capability = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        nested_profile,
    )["token"]
    nested_bwrap = exec_command(
        socket_path,
        nested_capability,
        pane_id,
        "/usr/bin/bwrap",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--ro-bind-try",
        "/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--unshare-user",
        "--unshare-pid",
        "--as-pid-1",
        "--die-with-parent",
        "--",
        "/bin/true",
    )
    assert_done(socket_path, nested_capability, nested_bwrap)

    landlock_nested = exec_command(
        socket_path,
        strict_capability["token"],
        pane_id,
        "/usr/bin/bwrap",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/bin",
        "/bin",
        "--ro-bind-try",
        "/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--unshare-user",
        "--unshare-pid",
        "--as-pid-1",
        "--die-with-parent",
        "--",
        "/bin/true",
    )
    assert_done(
        socket_path,
        strict_capability["token"],
        landlock_nested,
        expected_code=1,
    )
    landlock_nested_events = events(
        socket_path,
        strict_capability["token"],
        landlock_nested["command_id"],
    )
    assert b"setting up uid map" in decode_stream(
        landlock_nested_events, "stderr"
    )

    runtime_script = (
        "import json,os,ssl,subprocess,threading\n"
        "seen=[]\n"
        "t=threading.Thread(target=lambda:seen.append('thread'))\n"
        "t.start();t.join()\n"
        "subprocess.run(['/bin/sh','-c','exit 0'],check=True)\n"
        "os.stat('.');os.access('.',os.R_OK)\n"
        "r,w=os.pipe();os.write(w,b'x');os.close(w);assert os.read(r,1)==b'x';os.close(r)\n"
        "assert ssl.OPENSSL_VERSION\n"
        "print('STRICT_RUNTIME_PYTHON_OK')"
    )
    runtime_python = exec_command(
        socket_path,
        strict_capability["token"],
        pane_id,
        "/usr/bin/python3",
        "-c",
        runtime_script,
    )
    assert_done(socket_path, strict_capability["token"], runtime_python)
    runtime_python_events = events(
        socket_path,
        strict_capability["token"],
        runtime_python["command_id"],
    )
    assert (
        decode_stream(runtime_python_events, "stdout")
        == b"STRICT_RUNTIME_PYTHON_OK\n"
    )

    if landlock_expected:
        rust_runtime = exec_command(
            socket_path,
            strict_capability["token"],
            pane_id,
            str(runtime_init),
            "--version",
        )
        assert_done(socket_path, strict_capability["token"], rust_runtime)
        rust_runtime_events = events(
            socket_path,
            strict_capability["token"],
            rust_runtime["command_id"],
        )
        rust_output = decode_stream(
            rust_runtime_events, "stdout"
        ) + decode_stream(rust_runtime_events, "stderr")
        assert b"qwe-sandbox-init" in rust_output, rust_output

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

    fd_script = (
        "import json,os;"
        "fds={};"
        "\nfor fd in os.listdir('/proc/self/fd'):"
        "\n try: fds[fd]=os.readlink('/proc/self/fd/'+fd)"
        "\n except OSError: pass"
        "\nprint(json.dumps(fds,sort_keys=True))"
    )
    fd_command = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        fd_script,
    )
    assert_done(socket_path, read_only_capability, fd_command)
    fd_events = events(socket_path, read_only_capability, fd_command["command_id"])
    inherited_fds = json.loads(decode_stream(fd_events, "stdout"))
    assert set(inherited_fds) <= {"0", "1", "2"}, inherited_fds

    fifo_path = workspace / "credential-channel.fifo"
    fifo_worker, fifo_data = fifo_capture(fifo_path)
    fifo_script = (
        "import os;"
        "secret=os.read(3,16384);"
        f"fd=os.open({str(fifo_path)!r},os.O_WRONLY);"
        "os.write(fd,secret);"
        "os.close(fd)"
    )
    fifo_command = exec_command(
        socket_path,
        rw_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        fifo_script,
        credential="CMD_KEY",
    )
    assert_done(socket_path, rw_capability, fifo_command)
    fifo_worker.join(4)
    assert fifo_data == [COMMAND_SECRET], fifo_data
    fifo_path.unlink()

    persisted_credential = workspace / "credential-workspace-persistence.bin"
    credential_persistence_command = exec_command(
        socket_path,
        rw_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        (
            "import os,pathlib;"
            "pathlib.Path('credential-workspace-persistence.bin')"
            ".write_bytes(os.read(3,16384))"
        ),
        credential="CMD_KEY",
    )
    credential_persistence_status = started_sandbox(
        socket_path,
        rw_capability,
        credential_persistence_command["command_id"],
    )
    persistence_risk = credential_persistence_status["sandbox"]["risk"]
    assert persistence_risk["writable_workspace"] is True, persistence_risk
    assert persistence_risk["credential_exposed"] is True, persistence_risk
    assert (
        persistence_risk["credential_workspace_persistence"] is True
    ), persistence_risk
    assert_done(socket_path, rw_capability, credential_persistence_command)
    assert persisted_credential.read_bytes() == COMMAND_SECRET
    persisted_credential.unlink()

    protected_git = workspace / ".git"
    protected_target = protected_git / "sandbox-blocked"
    hardlink_source = protected_git / "hardlink-source"
    hardlink_source.write_text("ORIGINAL")
    hardlink_alias = workspace / "hardlink-alias"
    os.link(hardlink_source, hardlink_alias)
    protected_profile = profile(
        workspace,
        workspace_mode="read_write",
        protected_paths=[protected_git],
    )
    hardlink_issue = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        protected_profile,
    )
    assert hardlink_issue.get("code") == "sandbox_profile_unsupported", hardlink_issue
    assert "hardlinked file" in hardlink_issue["message"], hardlink_issue
    hardlink_alias.unlink()
    hardlink_source.unlink()

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
    protected_link.symlink_to(".git")
    symlink_hardlink_source = protected_git / "symlink-hardlink-source"
    symlink_hardlink_source.write_text("ORIGINAL")
    symlink_hardlink_alias = workspace / "symlink-hardlink-alias"
    os.link(symlink_hardlink_source, symlink_hardlink_alias)
    linked_profile = profile(
        workspace,
        workspace_mode="read_write",
        protected_paths=[protected_link],
    )
    linked_hardlink_issue = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        linked_profile,
    )
    assert (
        linked_hardlink_issue.get("code") == "sandbox_profile_unsupported"
    ), linked_hardlink_issue
    assert "hardlinked file" in linked_hardlink_issue["message"], linked_hardlink_issue
    symlink_hardlink_alias.unlink()
    symlink_hardlink_source.unlink()

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

    absolute_protected_link = workspace / "absolute-protected-link"
    absolute_protected_link.symlink_to(protected_git)
    absolute_link_profile = profile(
        workspace,
        workspace_mode="read_write",
        protected_paths=[absolute_protected_link],
    )
    absolute_link_issue = issue_capability(
        socket_path,
        management,
        principal_id,
        pane_id,
        absolute_link_profile,
    )
    assert (
        absolute_link_issue.get("code") == "sandbox_profile_unsupported"
    ), absolute_link_issue
    absolute_protected_link.unlink()

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

    proc_escape_target = Path(socket_path).parent / "proc-root-escape.txt"
    proc_escape_target.write_text("PROC_ROOT_ESCAPED")
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
            f"test -e /proc/{pane['pid']} && "
            f"if test -r /proc/{pane['worker_pid']}/root{proc_escape_target}; "
            "then printf 'PROC_ROOT_EXPOSED'; exit 97; "
            "else printf 'PID_NAMESPACE_SHARED'; fi"
        ),
    )
    assert_done(socket_path, shared_capability, shared_command)
    shared_events = events(
        socket_path,
        shared_capability,
        shared_command["command_id"],
    )
    assert decode_stream(shared_events, "stdout") == b"PID_NAMESPACE_SHARED"
    proc_escape_target.unlink()

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

    abstract_listener, abstract_worker, abstract_data, abstract_name = unix_capture()
    abstract_inherited_script = (
        "import socket;"
        f"s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);"
        f"s.connect({abstract_name!r});"
        "s.sendall(b'ABSTRACT_INHERITED');"
        "s.close()"
    )
    abstract_inherited = exec_command(
        socket_path,
        inherited_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        abstract_inherited_script,
    )
    assert_done(socket_path, inherited_capability, abstract_inherited)
    abstract_worker.join(4)
    abstract_listener.close()
    assert abstract_data == [b"ABSTRACT_INHERITED"], abstract_data

    abstract_listener, abstract_worker, abstract_data, abstract_name = unix_capture()
    abstract_isolated_script = (
        "import socket,sys;"
        f"s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);"
        "s.settimeout(1);"
        f"r=s.connect_ex({abstract_name!r});"
        "print('ABSTRACT_ISOLATED' if r else 'ABSTRACT_LEAKED');"
        "sys.exit(0 if r else 97)"
    )
    abstract_isolated = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        abstract_isolated_script,
    )
    assert_done(socket_path, read_only_capability, abstract_isolated)
    abstract_worker.join(4)
    abstract_listener.close()
    assert not abstract_data, abstract_data

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

    if landlock_expected:
        landlock_command = exec_command(
            socket_path,
            read_only_capability,
            pane_id,
            "/bin/sh",
            "-c",
            "printf '%s' \"${QWE_LANDLOCK_ABI:-missing}\"",
        )
        assert_done(socket_path, read_only_capability, landlock_command)
        landlock_events = events(
            socket_path,
            read_only_capability,
            landlock_command["command_id"],
        )
        assert decode_stream(landlock_events, "stdout") == str(
            audit["landlock_abi"]
        ).encode()

    non_pty_tiocsti_script = (
        "import ctypes,sys;"
        "libc=ctypes.CDLL(None,use_errno=True);"
        "value=ctypes.create_string_buffer(b'X');"
        "result=libc.ioctl(0,0x5412,value);"
        "error=ctypes.get_errno();"
        "print(f'NON_PTY_TIOCSTI_RESULT={result}:{error}',flush=True);"
        "sys.exit(0 if result == -1 and error == 1 else 97)"
    )
    non_pty_tiocsti = exec_command(
        socket_path,
        read_only_capability,
        pane_id,
        "/usr/bin/python3",
        "-c",
        non_pty_tiocsti_script,
    )
    assert_done(socket_path, read_only_capability, non_pty_tiocsti)
    non_pty_tiocsti_events = events(
        socket_path,
        read_only_capability,
        non_pty_tiocsti["command_id"],
    )
    assert (
        decode_stream(non_pty_tiocsti_events, "stdout")
        == b"NON_PTY_TIOCSTI_RESULT=-1:1\n"
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
        "sandboxed_pane_capability": "pass",
        "sandboxed_pane_runtime": (
            "pass" if sandboxed_pane_capability is not None else "unavailable"
        ),
        "sandboxed_pane_restricted_output": "pass",
        "sandboxed_pane_input_fail_closed": "pass",
        "pane_input_trust_floor": "pass",
        "fd_bind_probe": "pass",
        "strict_seccomp": "pass",
        "nested_bubblewrap": "pass",
        "nested_bubblewrap_landlock_denied": "pass",
        "strict_runtime_compatibility": "pass",
        "landlock_profile_modes": "pass",
        "request_cannot_enable": "pass",
        "request_cannot_downgrade": "pass",
        "workspace_ro_rw": "pass",
        "fd_inheritance": "pass",
        "credential_workspace_fifo_exfil_confirmed": "pass",
        "credential_workspace_persistence_confirmed": "pass",
        "protected_path": "pass",
        "protected_hardlink": "pass",
        "symlink_protected_hardlink": "pass",
        "symlink_protection": "pass",
        "absolute_protected_symlink_rejected": "pass",
        "home_read_only": "pass",
        "process_namespace_shared": "pass",
        "proc_root_bounded": "pass",
        "network_inherit": "pass",
        "network_inherit_abstract_socket": "pass",
        "credential_network_exfil_confirmed": "pass",
        "sandbox_risk_audit": "pass",
        "sandbox_audit_persistence": "pass",
        "sandbox_risk_persistence": "pass",
        "home_and_tmp": "pass",
        "environment_and_pid_isolation": "pass",
        "filesystem_boundary": "pass",
        "network_isolation": "pass",
        "network_isolate_abstract_socket": "pass",
        "loopback": "pass",
        "setuid_no_new_privs": "pass",
        "landlock_defense_in_depth": (
            "pass" if landlock_expected else "unavailable"
        ),
        "tiocsti_non_pty": "pass",
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

    sandboxed_session_id = "stage10-sandboxed-pane"
    sandboxed_restart_status = request(
        socket_path,
        "SESSION_STATUS",
        {"session_id": sandboxed_session_id},
    )
    sandboxed_restart = "unavailable"
    if sandboxed_restart_status.get("code") != "not_found":
        layout = request(
            socket_path,
            "SESSION_LAYOUT",
            {"session_id": sandboxed_session_id, "rows": 24, "columns": 80},
        )
        assert layout.get("panes"), layout
        recovered_profiles = []
        recovered_audits = []
        for layout_pane in layout["panes"]:
            pane_status = request(
                socket_path,
                "PANE_STATUS",
                {"pane_id": layout_pane["pane_id"]},
            )
            assert pane_status["sandbox"]["enforced"] is True, pane_status
            recovered_profiles.append(pane_status["sandbox"]["profile"])
            recovered_audits.append(pane_status["sandbox"]["audit"])
        assert all(profile == recovered_profiles[0] for profile in recovered_profiles)
        assert all(audit == recovered_audits[0] for audit in recovered_audits)
        assert recovered_audits[0]["fd_bind"] is True, recovered_audits
        sandboxed_worker_pid = request(
            socket_path,
            "PANE_STATUS",
            {"pane_id": layout["panes"][0]["pane_id"]},
        )["worker_pid"]
        sandboxed_generation = claim(socket_path, sandboxed_session_id, OWNER)
        sandboxed_closed = request(
            socket_path,
            "SESSION_KILL",
            {
                "session_id": sandboxed_session_id,
                "owner": OWNER,
                "generation": sandboxed_generation,
            },
        )
        assert sandboxed_closed.get("state") == "closed", sandboxed_closed
        wait_for(
            lambda: process_gone(sandboxed_worker_pid),
            "sandboxed Pane worker survived post-restart teardown",
        )
        sandboxed_restart = "pass"

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
        "sandboxed_pane_restart": sandboxed_restart,
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


def run_backend_incompatible_probe(qwed, root):
    socket_path = str(root / "qwed-incompatible-bwrap.sock")
    log_path = root / "qwed-incompatible-bwrap.log"
    fake_bwrap = root / "incompatible-bwrap"
    fake_bwrap.write_text(
        """#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  printf 'bubblewrap 0.4.0\\n'
  exit 0
fi
if [ "${1:-}" = "--help" ]; then
  printf '%s\\n' \
    '  --unshare-user' \
    '  --ro-bind SRC DEST' \
    '  --proc DEST' \
    '  --dev DEST'
  exit 0
fi
exit 1
"""
    )
    fake_bwrap.chmod(0o700)
    env = os.environ.copy()
    env["QWE_BWRAP_PATH"] = str(fake_bwrap)
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
                "session_id": "stage10-incompatible-bwrap",
                "program": "/bin/sh",
                "args": ["-c", "exec /bin/sleep 120"],
                "owner": OWNER,
            },
        )
        assert created.get("running") is True, created
        result = issue_capability(
            socket_path,
            management,
            principal_id,
            created["pane_id"],
            profile(root),
        )
        assert result.get("code") == "sandbox_backend_unavailable", result
        assert (
            result["details"]["reason"] == "bwrap_version_incompatible"
        ), result
        assert "missing required option" in result["details"]["diagnostics"][
            "stderr"
        ], result
    finally:
        stop_qwed(process, socket_path)


def run_sandbox_init_missing_probe(qwed, root):
    if not landlock_available():
        return False
    qwed = Path(qwed).resolve()
    bin_dir = root / "init-bin"
    bin_dir.mkdir()
    for source in (
        qwed,
        qwed.with_name("qwe-worker"),
        qwed.with_name("qwe-session-exec"),
    ):
        shutil.copy2(source, bin_dir / source.name)
    assert not (bin_dir / "qwe-sandbox-init").exists()

    socket_path = str(root / "init.sock")
    log_path = root / "init.log"
    env = os.environ.copy()
    env["QWE_BWRAP_PATH"] = shutil.which("bwrap")
    process = start_qwed(bin_dir / "qwed", socket_path, log_path, env=env)
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
                "session_id": "stage10-missing-sandbox-init",
                "program": "/bin/sh",
                "args": ["-c", "exec /bin/sleep 120"],
                "owner": OWNER,
            },
        )
        assert created.get("running") is True, created
        result = issue_capability(
            socket_path,
            management,
            principal_id,
            created["pane_id"],
            profile(root),
        )
        assert result.get("code") == "sandbox_backend_unavailable", result
        assert (
            result["details"]["reason"] == "sandbox_init_unavailable"
        ), result
        assert (
            result["details"]["diagnostics"]["failed_stage"]
            == "landlock_init"
        ), result
    finally:
        stop_qwed(process, socket_path)
    return True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    if not sys.platform.startswith("linux") or shutil.which("bwrap") is None:
        print(
            json.dumps(
                {
                    "case": (
                        "SANDBOX-004/SANDBOX-005/SANDBOX-006/"
                        "SANDBOX-007/SANDBOX-008/SANDBOX-009/"
                        "SANDBOX-010/SANDBOX-011/SANDBOX-012"
                    ),
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
            Path(args.qwed).with_name("qwe-sandbox-init"),
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
        run_backend_incompatible_probe(args.qwed, root)
        results["backend_feature_probe"] = "pass"
        results["sandbox_init_fail_closed"] = (
            "pass"
            if run_sandbox_init_missing_probe(args.qwed, root)
            else "unavailable"
        )
        print(
            json.dumps(
                {
                    "case": (
                        "SANDBOX-004/SANDBOX-005/SANDBOX-006/"
                        "SANDBOX-007/SANDBOX-008/SANDBOX-009/"
                        "SANDBOX-010/SANDBOX-011/SANDBOX-012"
                    ),
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
