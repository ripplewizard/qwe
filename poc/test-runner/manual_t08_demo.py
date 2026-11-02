#!/usr/bin/env python3
"""Run a human-observable full-screen T08 detach/reattach scenario."""

from __future__ import annotations

import argparse
import os
import signal
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

from terminal_interaction_smoke import (
    input_bytes,
    request,
    snapshot,
    start_session,
    status,
    success,
    wait_for,
    wait_ready,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--language", required=True, choices=("rust", "go", "cpp"))
    parser.add_argument("--results-dir", required=True, type=Path)
    parser.add_argument("--hold-seconds", type=float, default=5)
    args = parser.parse_args()

    args.results_dir.mkdir(parents=True, exist_ok=True)
    fixture = Path(tempfile.gettempdir()) / "qwe-t08-manual-large.txt"
    fixture.write_text(
        "".join(f"line-{line}\n" for line in range(1, 301)),
        encoding="utf-8",
    )
    session_id = f"manual-t08-{uuid.uuid4().hex}"
    socket_path = Path(f"/tmp/qwe-manual-t08-{uuid.uuid4().hex}.sock")
    database = args.results_dir / f"manual-t08-{args.language}.sqlite"
    daemon_log = args.results_dir / f"manual-t08-{args.language}.daemon.log"
    pty_log = args.results_dir / f"manual-t08-{args.language}.pty.log"
    snapshot_path = args.results_dir / f"manual-t08-{args.language}.snapshot.bin"
    environment = {**os.environ, "TERM": "xterm-256color", "QWE_LEASE_MS": "60000"}

    with daemon_log.open("w", encoding="utf-8") as daemon_output:
        daemon = subprocess.Popen(
            [str(args.binary), "daemon", str(socket_path), str(database)],
            stdout=daemon_output,
            stderr=subprocess.STDOUT,
            env=environment,
            text=True,
        )
        try:
            wait_ready(socket_path, daemon)
            start_session(
                socket_path,
                session_id,
                "manual-client",
                pty_log,
                "nvim",
                "-u",
                "NONE",
                '+set number',
                str(fixture),
            )
            input_bytes(
                socket_path,
                session_id,
                "manual-client",
                (
                    b':lua vim.defer_fn(function() '
                    b'vim.api.nvim_echo({{"QWE_T08_DETACHED_OUTPUT","ErrorMsg"}}, '
                    b'true, {}) end, 700)\r'
                ),
            )
            input_bytes(socket_path, session_id, "manual-client", b"G")
            wait_for(
                socket_path,
                session_id,
                lambda _, data: b"line-300" in data and b"300,1" in data,
                "Neovim line 300",
            )
            detached = success(
                request(
                    socket_path,
                    "SESSION_DETACH",
                    session_id,
                    "manual-client",
                ),
                "SESSION_DETACH",
            )
            if detached.get("attached") is not False:
                raise RuntimeError("session did not detach")
            detached_status = wait_for(
                socket_path,
                session_id,
                lambda response, data: (
                    response.get("running") is True
                    and response.get("attached") is False
                    and b"QWE_T08_DETACHED_OUTPUT" in data
                ),
                "detached output",
                timeout=4,
            )
            reattached = success(
                request(
                    socket_path,
                    "SESSION_ATTACH",
                    session_id,
                    "manual-client",
                ),
                "SESSION_ATTACH",
            )
            if reattached.get("attached") is not True:
                raise RuntimeError("session did not reattach")
            input_bytes(socket_path, session_id, "manual-client", b"gg\x04")
            input_bytes(
                socket_path,
                session_id,
                "manual-client",
                b':echo "QWE_T08_REATTACHED_INPUT"\r',
            )
            final = wait_for(
                socket_path,
                session_id,
                lambda _, data: b"QWE_T08_REATTACHED_INPUT" in data,
                "post-reattach echo",
            )
            data = snapshot(final)
            snapshot_path.write_bytes(data)
            print(
                "T08 manual replay: "
                f"language={args.language} pty_running={detached_status['running']} "
                f"detached_output={b'QWE_T08_DETACHED_OUTPUT' in data} "
                f"reattach_input={b'QWE_T08_REATTACHED_INPUT' in data} "
                f"bytes={len(data)}",
                flush=True,
            )
            os.write(1, data)
            os.write(1, b"\r\n")
            time.sleep(max(0, args.hold_seconds))
        finally:
            if daemon.poll() is None:
                daemon.send_signal(signal.SIGTERM)
                try:
                    daemon.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait(timeout=2)
            if socket_path.exists():
                socket_path.unlink()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
