#!/usr/bin/env python3

import argparse
import fcntl
import json
import os
import pty
import select
import socket
import struct
import subprocess
import termios
import tempfile
import time
from pathlib import Path

from stage7_resource_probe import (
    claim,
    crash_qwed,
    receive_exact,
    request,
    start_qwed,
    stop_qwed,
    wait_for,
)


OWNER = "stage8-layout"
FRAME_INPUT = 0x02
FRAME_DETACH = 0x05
FRAME_ATTACH_OPEN = 0x01
FRAME_ATTACH_ACCEPTED = 0x81
FRAME_SNAPSHOT = 0x82


def attach_pane(path, session_id, pane_id, rows, columns, generation):
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stream.settimeout(10)
    stream.connect(f"{path}.attach")
    opened = json.dumps(
        {
            "protocol_version": 1,
            "session_id": session_id,
            "pane_id": pane_id,
            "owner": OWNER,
            "lease_generation": generation,
            "rows": rows,
            "columns": columns,
            "mode": "exclusive",
        },
        separators=(",", ":"),
    ).encode()
    stream.sendall(struct.pack(">BI", FRAME_ATTACH_OPEN, len(opened)))
    stream.sendall(opened)
    accepted = receive_attach_frame(stream)
    assert accepted[0] == FRAME_ATTACH_ACCEPTED, accepted
    assert accepted[1]["pane_id"] == pane_id, accepted
    snapshot = receive_attach_frame(stream)
    assert snapshot[0] == FRAME_SNAPSHOT, snapshot
    return stream


def receive_attach_frame(stream):
    frame_type = receive_exact(stream, 1)[0]
    length = struct.unpack(">I", receive_exact(stream, 4))[0]
    payload = receive_exact(stream, length)
    if frame_type == 0x87:
        raise RuntimeError(f"attach error: {payload.decode(errors='replace')}")
    return frame_type, json.loads(payload) if payload else {}


def send_raw_frame(stream, frame_type, payload):
    stream.sendall(struct.pack(">BI", frame_type, len(payload)))
    stream.sendall(payload)


def close_attach(stream):
    if stream is None:
        return
    try:
        send_raw_frame(
            stream,
            FRAME_DETACH,
            json.dumps(
                {"protocol_version": 1, "hold_ttl_ms": 0},
                separators=(",", ":"),
            ).encode(),
        )
    except OSError:
        pass
    stream.close()


def read_until(master, output, marker, timeout=8, start=0):
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
        if marker in output[start:]:
            return
    raise RuntimeError(f"compositor output did not contain {marker!r}: {output[-4096:]!r}")


def drain(master, output, timeout=1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        readable, _, _ = select.select([master], [], [], 0.05)
        if not readable:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError:
            return
        if not chunk:
            return
        output.extend(chunk)


def wait_file_contains(path, marker, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and marker in path.read_text(errors="replace"):
            return
        time.sleep(0.02)
    content = path.read_text(errors="replace") if path.exists() else ""
    raise RuntimeError(
        f"clipboard file {path} did not contain {marker!r}: {content[:2000]!r}"
    )


def probe_compositor(cli, socket_path, clipboard_path):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    process = subprocess.Popen(
        [
            cli,
            "--socket",
            socket_path,
            "--owner",
            OWNER,
            "session",
            "compose",
            "layout-session",
        ],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        close_fds=True,
        start_new_session=True,
        env={
            **os.environ,
            "QWE_CLIPBOARD_COMMAND": f"cat > {clipboard_path}",
        },
    )
    os.close(slave)
    os.set_blocking(master, False)
    output = bytearray()
    try:
        read_until(master, output, b"PANE_TWO_READY")
        read_until(master, output, b"QWE layout-s")
        assert b"\x1b[?1002h\x1b[?1006h" in output, output[:4096]
        start = len(output)
        os.write(master, b"\x01n")
        read_until(master, output, b"focus=2/2", start=start)
        os.write(master, b"printf 'COMPOSE_TWO\\n'\r")
        read_until(master, output, b"COMPOSE_TWO")
        os.write(master, b"printf 'COPY_TARGET\\n'\r")
        read_until(master, output, b"COPY_TARGET")
        os.write(
            master,
            b"for i in $(seq 1 60); do echo SCROLL_$i; done\r",
        )
        read_until(master, output, b"SCROLL_60")
        start = len(output)
        os.write(master, b"\x01[")
        read_until(master, output, b"COPY row=", start=start)
        start = len(output)
        os.write(master, b"\x15" * 20 + b"Ay")
        read_until(master, output, b"copied ", start=start)
        wait_file_contains(clipboard_path, "SCROLL_14")
        assert "SCROLL_60" not in clipboard_path.read_text(errors="replace")
        start = len(output)
        os.write(master, b"\x01[")
        read_until(master, output, b"COPY row=", start=start)
        start = len(output)
        os.write(
            master,
            b"\x1b[<0;51;2M\x1b[<32;61;5M\x1b[<0;61;5m",
        )
        read_until(master, output, b"copied ", start=start)
        wait_file_contains(clipboard_path, "SCROLL_34")
        start = len(output)
        os.write(master, b"\x01m")
        read_until(master, output, b"MENU", start=start)
        os.write(master, b"q")
        os.write(master, b"printf 'MENU_SAFE\\n'\r")
        read_until(master, output, b"MENU_SAFE", start=start)
        assert b"qprintf" not in output[start:], output[start:]
        start = len(output)
        os.write(master, b"\x01c")
        read_until(master, output, b"focus=3/3", start=start)
        os.write(master, b"printf 'COMPOSE_THREE\\n'\r")
        read_until(master, output, b"COMPOSE_THREE")
        start = len(output)
        os.write(master, b"\x01x")
        read_until(master, output, b"focus=2/2", start=start)
        start = len(output)
        os.write(master, b"\x01h")
        read_until(master, output, b"focus=1/2", start=start)
        start = len(output)
        os.write(master, b"\x01f")
        read_until(master, output, b"[float]", start=start)
        read_until(master, output, b"focus=3/3", start=start)
        os.write(master, b"printf 'COMPOSE_FLOAT\\n'\r")
        read_until(master, output, b"COMPOSE_FLOAT", start=start)
        start = len(output)
        os.write(master, b"\x01d")
        read_until(master, output, b"MOVE FLOAT", start=start)
        start = len(output)
        os.write(master, b"hhLy")
        read_until(master, output, b"floating bounds saved", start=start)
        floating_layout = request(
            socket_path,
            "SESSION_LAYOUT",
            {"session_id": "layout-session", "rows": 29, "columns": 100},
        )
        floating = next(
            pane for pane in floating_layout["panes"] if pane["kind"] == "floating"
        )
        assert floating["rect"] == {
            "x": 8,
            "y": 4,
            "rows": 21,
            "columns": 81,
        }, floating
        start = len(output)
        os.write(
            master,
            b"\x1b[<0;9;5M\x1b[<32;12;7M\x1b[<0;12;7m",
        )
        read_until(master, output, b"floating bounds saved by mouse", start=start)
        dragged_layout = request(
            socket_path,
            "SESSION_LAYOUT",
            {"session_id": "layout-session", "rows": 29, "columns": 100},
        )
        dragged = next(
            pane for pane in dragged_layout["panes"] if pane["kind"] == "floating"
        )
        assert dragged["rect"] == {
            "x": 11,
            "y": 6,
            "rows": 21,
            "columns": 81,
        }, dragged
        start = len(output)
        os.write(
            master,
            b"\x1b[<0;92;27M\x1b[<32;94;28M\x1b[<0;94;28m",
        )
        read_until(master, output, b"floating bounds resized by mouse", start=start)
        resized_layout = request(
            socket_path,
            "SESSION_LAYOUT",
            {"session_id": "layout-session", "rows": 29, "columns": 100},
        )
        resized = next(
            pane for pane in resized_layout["panes"] if pane["kind"] == "floating"
        )
        assert resized["rect"] == {
            "x": 11,
            "y": 6,
            "rows": 22,
            "columns": 83,
        }, resized
        start = len(output)
        os.write(master, b"\x01f")
        read_until(master, output, b"focus=4/4", start=start)
        stacked_layout = request(
            socket_path,
            "SESSION_LAYOUT",
            {"session_id": "layout-session", "rows": 29, "columns": 100},
        )
        stacked_floats = [
            pane for pane in stacked_layout["panes"] if pane["kind"] == "floating"
        ]
        assert stacked_floats[0]["pane_id"] == floating["pane_id"], stacked_layout
        assert stacked_floats[1]["z_index"] > stacked_floats[0]["z_index"], stacked_layout
        start = len(output)
        os.write(master, b"\x1b[<0;93;26M\x1b[<0;93;26m")
        read_until(master, output, b"floating Pane raised", start=start)
        raised_layout = request(
            socket_path,
            "SESSION_LAYOUT",
            {"session_id": "layout-session", "rows": 29, "columns": 100},
        )
        raised_floats = [
            pane for pane in raised_layout["panes"] if pane["kind"] == "floating"
        ]
        assert raised_floats[0]["pane_id"] == stacked_floats[1]["pane_id"], raised_layout
        assert raised_floats[1]["pane_id"] == floating["pane_id"], raised_layout
        assert raised_floats[1]["z_index"] > raised_floats[0]["z_index"], raised_layout
        start = len(output)
        os.write(master, b"\x01x")
        read_until(master, output, b"focus=3/3", start=start)
        start = len(output)
        os.write(master, b"\x01x")
        read_until(master, output, b"focus=2/2", start=start)
        os.write(master, b"\x1d")
        drain(master, output, timeout=2)
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired as failure:
            drain(master, output)
            raise RuntimeError(
                f"compositor did not exit after detach: {output[-4096:]!r}"
            ) from failure
        drain(master, output)
        assert process.returncode == 0, (process.returncode, output[-4096:])
        assert b"\x1b[=0u" in output, output[-4096:]
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qwed", required=True)
    parser.add_argument("--cli", required=True)
    parser.add_argument("--socket", required=True)
    args = parser.parse_args()

    qwed = None
    attachments = []
    log_path = Path(f"/tmp/qwe-stage8-layout-{os.getpid()}.log")
    try:
        with tempfile.TemporaryDirectory(prefix="qwe-stage8-layout-") as _log_dir:
            qwed = start_qwed(args.qwed, args.socket, log_path)
            created = request(
                args.socket,
                "SESSION_CREATE",
                {
                    "session_id": "layout-session",
                    "program": "sh",
                    "args": ["-c", "printf 'PANE_ONE_READY\\n'; exec sleep 120"],
                },
            )
            assert created.get("running") is True, created
            generation = claim(args.socket, "layout-session", OWNER)
            second = request(
                args.socket,
                "PANE_CREATE",
                {
                    "session_id": "layout-session",
                    "program": "sh",
                    "args": ["-c", "printf 'PANE_TWO_READY\\n'; exec sh -i"],
                    "owner": OWNER,
                    "generation": generation,
                },
            )
            assert second.get("running") is True, second
            pane_ids = [created["pane_id"], second["pane_id"]]

            layout = request(
                args.socket,
                "SESSION_LAYOUT",
                {"session_id": "layout-session", "rows": 20, "columns": 81},
            )
            assert [pane["pane_id"] for pane in layout["panes"]] == pane_ids, layout
            rects = [pane["rect"] for pane in layout["panes"]]
            assert rects == [
                {"x": 0, "y": 0, "rows": 20, "columns": 40},
                {"x": 40, "y": 0, "rows": 20, "columns": 41},
            ], rects
            assert [pane["layout_order"] for pane in layout["panes"]] == [0, 1], layout
            assert [pane["layout_weight"] for pane in layout["panes"]] == [1, 1], layout
            assert layout["panes"][0]["focused"] is True, layout

            for pane_id in pane_ids:
                attachments.append(
                    attach_pane(
                        args.socket,
                        "layout-session",
                        pane_id,
                        20,
                        40,
                        generation,
                    )
                )

            send_raw_frame(
                attachments[1],
                FRAME_INPUT,
                b"printf 'PANE_TWO_INPUT\\n'\n",
            )
            wait_for(
                lambda: (
                    snapshot
                    if "PANE_TWO_INPUT"
                    in (
                        snapshot := request(
                            args.socket,
                            "PANE_ATTACH",
                            {"pane_id": pane_ids[1]},
                        )
                    ).get("screen", "")
                    else None
                ),
                "Pane-targeted INPUT was not delivered to the selected Pane",
            )

            for attachment in attachments:
                close_attach(attachment)
            attachments.clear()

            clipboard_path = Path(f"/tmp/qwe-stage8-clipboard-{os.getpid()}.txt")
            clipboard_path.unlink(missing_ok=True)
            try:
                probe_compositor(args.cli, args.socket, clipboard_path)
            finally:
                clipboard_path.unlink(missing_ok=True)
            focused_layout = request(
                args.socket,
                "SESSION_LAYOUT",
                {"session_id": "layout-session", "rows": 20, "columns": 81},
            )
            assert [
                pane["pane_id"] for pane in focused_layout["panes"]
            ] == [pane_ids[1], pane_ids[0]], focused_layout
            assert focused_layout["panes"][0]["focused"] is True, focused_layout
            focus_generation = claim(args.socket, "layout-session", OWNER)
            focused_first = request(
                args.socket,
                "PANE_FOCUS",
                {
                    "pane_id": pane_ids[0],
                    "owner": OWNER,
                    "generation": focus_generation,
                },
            )
            assert focused_first.get("pane_id") == pane_ids[0], focused_first
            focused_second = request(
                args.socket,
                "PANE_FOCUS",
                {
                    "pane_id": pane_ids[1],
                    "owner": OWNER,
                    "generation": focus_generation,
                },
            )
            assert focused_second.get("pane_id") == pane_ids[1], focused_second

            crash_qwed(qwed)
            qwed = start_qwed(args.qwed, args.socket, log_path)
            recovered_layout = request(
                args.socket,
                "SESSION_LAYOUT",
                {"session_id": "layout-session", "rows": 20, "columns": 81},
            )
            assert [
                pane["pane_id"] for pane in recovered_layout["panes"]
            ] == [pane_ids[1], pane_ids[0]], recovered_layout
            assert [pane["layout_order"] for pane in recovered_layout["panes"]] == [0, 1]
            assert recovered_layout["panes"][0]["focused"] is True, recovered_layout

            killed = request(
                args.socket,
                "SESSION_KILL",
                {
                    "session_id": "layout-session",
                    "owner": OWNER,
                    "generation": focus_generation,
                },
            )
            assert killed.get("running") is False, killed
            print(
                json.dumps(
                    {
                        "case": "LAYOUT-001/LAYOUT-002/LAYOUT-003/LAYOUT-004/LAYOUT-005/LAYOUT-006/LAYOUT-007/LAYOUT-008/LAYOUT-009/LAYOUT-010/LAYOUT-011/LAYOUT-012/LAYOUT-013/LAYOUT-014/LAYOUT-015/LAYOUT-016/LAYOUT-017",
                        "deterministic_two_pane_tiling": "pass",
                        "pane_addressed_attach": "pass",
                        "concurrent_pane_attachments": "pass",
                        "pane_addressed_input": "pass",
                        "interactive_compositor": "pass",
                        "focus_switch": "pass",
                        "focus_restart_persistence": "pass",
                        "compose_split_close": "pass",
                        "compose_reorder": "pass",
                        "pane_clipped_copy": "pass",
                        "menu_keys_do_not_reach_pty": "pass",
                        "floating_pane": "pass",
                        "floating_move_persistence": "pass",
                        "mouse_drag": "pass",
                        "mouse_resize_handle": "pass",
                        "mouse_raise": "pass",
                        "scrollback_copy": "pass",
                        "mouse_copy": "pass",
                    },
                    separators=(",", ":"),
                )
            )
    except Exception:
        if log_path.exists():
            print(log_path.read_text(errors="replace"), flush=True)
        raise
    finally:
        for attachment in attachments:
            close_attach(attachment)
        stop_qwed(qwed, args.socket)


if __name__ == "__main__":
    main()
