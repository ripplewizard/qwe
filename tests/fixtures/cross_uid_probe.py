#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path


FIXED_CLIENT = Path("/tmp/qwe-cross-uid-client.py")
SOCKET_PATH_FILE = Path("/tmp/qwe-cross-uid-client.socket")


def run(arguments, *, check=True):
    return subprocess.run(
        [str(value) for value in arguments],
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def wait_socket(path, process):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(process.stderr.read().decode(errors="replace"))
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.connect(str(path))
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError("timed out waiting for qwed")


def run_client():
    result = run(
        ["sudo", "-n", "-u", "qwe-probe", FIXED_CLIENT],
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "qwe-probe helper is not allowlisted: "
            f"{result.stderr.strip()}"
        )
    return json.loads(result.stdout)


def add_acl(root, socket_path):
    if sys.platform == "darwin":
        run(
            [
                "chmod",
                "+a",
                "user:qwe-probe allow read,execute",
                root,
            ]
        )
        run(
            [
                "chmod",
                "+a",
                "user:qwe-probe allow read,write",
                socket_path,
            ]
        )
    else:
        run(["setfacl", "-m", "u:qwe-probe:rx", root])
        run(["setfacl", "-m", "u:qwe-probe:rw", socket_path])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()
    socket_path = Path(args.socket).resolve()
    root = socket_path.parent
    qwed = None
    shutil.copy2(
        Path(__file__).with_name("cross_uid_client.py"),
        FIXED_CLIENT,
    )
    FIXED_CLIENT.chmod(0o755)
    SOCKET_PATH_FILE.write_text(str(socket_path), encoding="utf-8")
    SOCKET_PATH_FILE.chmod(0o644)
    try:
        qwed = subprocess.Popen(
            [
                args.qwed,
                "--socket",
                socket_path,
                "--store",
                f"{socket_path}.sqlite",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        wait_socket(socket_path, qwed)

        denied = run_client()
        assert denied["result"] == "connect_denied", denied

        add_acl(root, socket_path)
        allowed = run_client()
        assert allowed["result"] == "connect_allowed", allowed
        assert allowed["uid"] != os.getuid(), allowed
        time.sleep(0.1)
        qwed.terminate()
        qwed.wait(timeout=5)
        stderr = qwed.stderr.read().decode(errors="replace")
        qwed = None
        assert "does not match current user" in stderr, stderr
        print(
            json.dumps(
                {
                    "case": "SEC-004",
                    "os_socket_permissions": "pass",
                    "application_peer_uid_check": "pass",
                    "peer_uid": allowed["uid"],
                },
                separators=(",", ":"),
            )
        )
    finally:
        if qwed is not None and qwed.poll() is None:
            os.killpg(qwed.pid, signal.SIGKILL)
            qwed.wait(timeout=5)
        SOCKET_PATH_FILE.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
