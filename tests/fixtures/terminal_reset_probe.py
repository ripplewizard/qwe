#!/usr/bin/env python3

import argparse
import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import termios
import time


def read_until(master, output, marker=None, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        readable, _, _ = select.select([master], [], [], 0.05)
        if readable:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                chunk = b""
            if not chunk:
                break
            output.extend(chunk)
        if marker is not None and marker in output:
            return True
    return marker is None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", required=True)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--session-id", required=True)
    args = parser.parse_args()

    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    process = subprocess.Popen(
        [
            args.cli,
            "--socket",
            args.socket,
            "--owner",
            "terminal-reset-probe",
            "session",
            "attach",
            args.session_id,
        ],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        close_fds=True,
        start_new_session=True,
    )
    os.close(slave)
    os.set_blocking(master, False)
    output = bytearray()
    try:
        time.sleep(0.5)
        os.write(
            master,
            b"printf '\\033[>1u'; printf 'KITTY_READY\\n'\r",
        )
        assert read_until(master, output, b"KITTY_READY"), output[-4096:]

        os.write(master, b"\x1d")
        assert read_until(master, output, b"\x1b[=0u"), output[-4096:]
        process.wait(timeout=5)

        assert b"\x1b[<1u" in output, output[-4096:]
        assert b"\x1b[=0u" in output, output[-4096:]
        assert b"\x1b[>4;0m" in output, output[-4096:]
        assert process.returncode == 0, process.returncode
        print(
            json.dumps(
                {
                    "case": "ATTACH-015",
                    "kitty_protocol_reset": "pass",
                    "modified_keys_reset": "pass",
                },
                separators=(",", ":"),
            )
        )
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)


if __name__ == "__main__":
    main()
