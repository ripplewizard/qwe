#!/usr/bin/env python3

import argparse
import json
import os
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path


SECRET = b"qwe-keychain-local-acl-test"


def run(arguments, *, check=True, input_text=None):
    return subprocess.run(
        [str(value) for value in arguments],
        check=check,
        text=True,
        input=input_text,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def wait_socket(path, process):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(
                f"qwed exited with {process.returncode}: "
                f"{process.stderr.read().decode(errors='replace')}"
            )
        result = run(
            ["target/debug/qwe", "--socket", path, "ping"],
            check=False,
        )
        if result.returncode == 0:
            return
        time.sleep(0.02)
    raise RuntimeError("timed out waiting for qwed")


def session_result(socket_path, session_id, credential):
    result = run(
        [
            "target/debug/qwe",
            "--socket",
            socket_path,
            "session",
            "create",
            session_id,
            "python3",
            "--credential",
            credential,
            "--",
            "-c",
            (
                "import os,time;"
                "fd=int(os.environ['QWE_CREDENTIAL_FD']);"
                "data=os.read(fd,1024);"
                f"print('KEYCHAIN_OK='+str(len(data)),flush=True);"
                "time.sleep(2)"
            ),
        ],
        check=False,
    )
    output = result.stdout.strip()
    return result.returncode, json.loads(output) if output else None, result.stderr


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    root = Path(tempfile.mkdtemp(prefix="qwe-keychain-smoke-", dir="/tmp"))
    keychain = root / "qwe-signing.keychain-db"
    password = "qwe-local-keychain-test"
    binary_dir = root / "bin"
    qwed = Path(args.qwed).resolve()
    qwe = Path("target/debug/qwe").resolve()
    worker = qwed.with_name("qwe-worker")
    launcher = qwed.with_name("qwe-session-exec")
    qwed_process = None
    original_keychains = None
    try:
        binary_dir.mkdir()
        for source in (qwed, worker, launcher):
            shutil.copy2(source, binary_dir / source.name)
        signed_qwed = binary_dir / "qwed"

        run(["security", "create-keychain", "-p", password, keychain])
        run(["security", "unlock-keychain", "-p", password, keychain])
        original_keychains = [
            line.strip().strip('"')
            for line in run(
                ["security", "list-keychains", "-d", "user"]
            ).stdout.splitlines()
            if line.strip()
        ]
        run(
            [
                "security",
                "list-keychains",
                "-d",
                "user",
                "-s",
                keychain,
                *original_keychains,
            ]
        )
        signing = run(
            [
                "codesign",
                "--force",
                "--sign",
                "-",
                signed_qwed,
            ],
            check=False,
        )
        assert signing.returncode == 0, signing.stderr
        signature = run(["codesign", "-dv", signed_qwed], check=False).stderr
        assert "Signature=adhoc" in signature, signature

        run(
            [
                "security",
                "add-generic-password",
                "-a",
                "qwe-test",
                "-s",
                "qwe.TEST_KEY",
                "-w",
                SECRET.decode(),
                "-T",
                signed_qwed,
                keychain,
            ]
        )
        qwed_process = subprocess.Popen(
            [
                signed_qwed,
                "--socket",
                args.socket,
                "--store",
                f"{args.socket}.sqlite",
                "--keychain-service-prefix",
                "qwe",
                "--keychain-path",
                keychain,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        wait_socket(args.socket, qwed_process)

        status, created, error = session_result(
            args.socket, "keychain-unlocked", "TEST_KEY"
        )
        assert status == 0, (status, created, error)
        assert created["running"] is True, created

        run(["security", "lock-keychain", keychain])
        status, locked, error = session_result(
            args.socket, "keychain-locked", "TEST_KEY"
        )
        assert status != 0, (locked, error)
        assert "credential_unavailable" in error, error
        assert run(
            ["target/debug/qwe", "--socket", args.socket, "ping"]
        ).returncode == 0

        run(["security", "unlock-keychain", "-p", password, keychain])
        status, unlocked, error = session_result(
            args.socket, "keychain-unlocked-again", "TEST_KEY"
        )
        assert status == 0, (status, unlocked, error)
        assert unlocked["running"] is True, unlocked

        print(
            json.dumps(
                {
                    "case": "CRED-003",
                    "explicit_adhoc_code_signature": "pass",
                    "keychain_acl_trusted_app": "pass",
                    "locked_keychain_fails_closed": "pass",
                    "unlocked_keychain_recovers": "pass",
                    "public_developer_id": "not-available",
                },
                separators=(",", ":"),
            )
        )
    finally:
        if qwed_process is not None and qwed_process.poll() is None:
            os.killpg(qwed_process.pid, signal.SIGTERM)
            try:
                qwed_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(qwed_process.pid, signal.SIGKILL)
                qwed_process.wait(timeout=5)
        if original_keychains:
            run(
                [
                    "security",
                    "list-keychains",
                    "-d",
                    "user",
                    "-s",
                    *original_keychains,
                ],
                check=False,
            )
        run(["security", "delete-keychain", keychain], check=False)
        shutil.rmtree(root, ignore_errors=True)
        for path in (
            Path(args.socket),
            Path(f"{args.socket}.attach"),
            Path(f"{args.socket}.sqlite"),
            Path(f"{args.socket}.sqlite-shm"),
            Path(f"{args.socket}.sqlite-wal"),
        ):
            path.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
