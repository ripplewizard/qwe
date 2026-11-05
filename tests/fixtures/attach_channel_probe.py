#!/usr/bin/env python3

import argparse
import json
import socket
import struct
import time


ATTACH_OPEN = 0x01
INPUT = 0x02
RESIZE = 0x03
ACK = 0x04
DETACH = 0x05
ATTACH_ACCEPTED = 0x81
SNAPSHOT = 0x82
OUTPUT = 0x83
RESIZED = 0x84
PROCESS_EXITED = 0x85
OWNERSHIP_REVOKED = 0x86
ERROR = 0x87

PROTOCOL_VERSION = 1


def send_json(stream, frame_type, value):
    payload = json.dumps(value, separators=(",", ":")).encode()
    send_payload(stream, frame_type, payload)


def send_payload(stream, frame_type, payload):
    stream.sendall(struct.pack(">BI", frame_type, len(payload)))
    stream.sendall(payload)


def receive_exact(stream, count):
    chunks = []
    remaining = count
    while remaining:
        chunk = stream.recv(remaining)
        if not chunk:
            raise EOFError("attach channel closed during a frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def receive_frame(stream):
    frame_type, length = struct.unpack(">BI", receive_exact(stream, 5))
    return frame_type, receive_exact(stream, length)


def open_frame(session_id, owner, mode="exclusive", version=PROTOCOL_VERSION):
    return {
        "protocol_version": version,
        "session_id": session_id,
        "owner": owner,
        "rows": 24,
        "columns": 80,
        "mode": mode,
    }


def connect(path, receive_buffer=None):
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    stream.settimeout(3)
    if receive_buffer is not None:
        stream.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, receive_buffer)
    stream.connect(path)
    return stream


def expect_error(stream, code, allow_output=False):
    while True:
        frame_type, payload = receive_frame(stream)
        if allow_output and frame_type == OUTPUT:
            continue
        break
    assert frame_type == ERROR, f"expected ERROR, received 0x{frame_type:02x}"
    failure = json.loads(payload)
    assert failure["code"] == code, failure
    stream.close()


def expect_eof(stream):
    while stream.recv(4096):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--session-id", required=True)
    args = parser.parse_args()
    attach_path = f"{args.socket}.attach"

    read_only = connect(attach_path)
    send_json(
        read_only,
        ATTACH_OPEN,
        open_frame(args.session_id, "read-only", mode="read_only"),
    )
    expect_error(read_only, "unsupported_mode")

    incompatible = connect(attach_path)
    send_json(
        incompatible,
        ATTACH_OPEN,
        open_frame(args.session_id, "old-client", version=99),
    )
    expect_error(incompatible, "incompatible_version")

    stream = connect(attach_path)
    send_json(stream, ATTACH_OPEN, open_frame(args.session_id, "channel-probe"))
    frame_type, payload = receive_frame(stream)
    assert frame_type == ATTACH_ACCEPTED, f"expected ATTACH_ACCEPTED, received {frame_type}"
    accepted = json.loads(payload)
    assert accepted["session_id"] == args.session_id
    assert len(accepted["reclaim_token"]) == 32

    frame_type, payload = receive_frame(stream)
    assert frame_type == SNAPSHOT, f"expected SNAPSHOT, received {frame_type}"
    snapshot = json.loads(payload)
    required_state = {
        "cursor_row",
        "cursor_column",
        "cursor_visible",
        "cursor_style",
        "alternate_screen",
        "application_cursor",
        "application_keypad",
        "bracketed_paste",
        "mouse_protocol_mode",
        "mouse_protocol_encoding",
        "next_stream_offset",
    }
    assert required_state.issubset(snapshot), snapshot

    second = connect(attach_path)
    send_json(second, ATTACH_OPEN, open_frame(args.session_id, "second-client"))
    expect_error(second, "already_attached")

    send_json(
        stream,
        RESIZE,
        {"protocol_version": PROTOCOL_VERSION, "rows": 40, "columns": 120},
    )
    command = (
        b"printf 'CHANNEL_%s\\n' BEGIN; sleep 0.05; "
        b"printf 'CHANNEL_%s\\n' END\r"
    )
    send_payload(stream, INPUT, command)

    expected_offset = snapshot["next_stream_offset"]
    raw_output = bytearray()
    resized = None
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        frame_type, payload = receive_frame(stream)
        if frame_type == OUTPUT:
            offset = struct.unpack(">Q", payload[:8])[0]
            assert offset == expected_offset, (offset, expected_offset)
            raw_output.extend(payload[8:])
            expected_offset += len(payload) - 8
            if b"CHANNEL_END" in raw_output:
                break
        elif frame_type == RESIZED:
            resized = json.loads(payload)
            assert resized["rows"] == 40
            assert resized["columns"] == 120
            assert resized["stream_offset"] >= snapshot["next_stream_offset"]
        elif frame_type in (PROCESS_EXITED, OWNERSHIP_REVOKED, ERROR):
            raise AssertionError(f"unexpected terminal frame 0x{frame_type:02x}")
    else:
        raise AssertionError("timed out waiting for live attach output")

    assert resized is not None, "resize was not acknowledged"
    assert raw_output.count(b"CHANNEL_BEGIN") == 1, raw_output
    assert raw_output.count(b"CHANNEL_END") == 1, raw_output

    send_json(
        stream,
        ACK,
        {"protocol_version": PROTOCOL_VERSION, "stream_offset": expected_offset},
    )
    send_payload(stream, 0x42, b"")
    expect_error(stream, "unknown_frame_type", allow_output=True)

    detach = connect(attach_path)
    send_json(detach, ATTACH_OPEN, open_frame(args.session_id, "detach-probe"))
    frame_type, _ = receive_frame(detach)
    assert frame_type == ATTACH_ACCEPTED
    frame_type, _ = receive_frame(detach)
    assert frame_type == SNAPSHOT
    send_json(
        detach,
        DETACH,
        {"protocol_version": PROTOCOL_VERSION, "hold_ttl_ms": 0},
    )
    expect_eof(detach)
    detach.close()

    slow = connect(attach_path, receive_buffer=4096)
    slow.settimeout(10)
    send_json(slow, ATTACH_OPEN, open_frame(args.session_id, "slow-client"))
    frame_type, _ = receive_frame(slow)
    assert frame_type == ATTACH_ACCEPTED
    frame_type, _ = receive_frame(slow)
    assert frame_type == SNAPSHOT
    flood = b"yes Z | head -c 4194304\r"
    send_payload(slow, INPUT, flood)
    time.sleep(5)
    expect_error(slow, "output_backpressure", allow_output=True)

    time.sleep(0.3)
    recovered = connect(attach_path)
    recovered.settimeout(10)
    send_json(recovered, ATTACH_OPEN, open_frame(args.session_id, "recovered-client"))
    frame_type, _ = receive_frame(recovered)
    assert frame_type == ATTACH_ACCEPTED
    frame_type, _ = receive_frame(recovered)
    assert frame_type == SNAPSHOT
    send_payload(recovered, INPUT, b"\x03")
    time.sleep(0.2)
    recovered.close()

    print(
        json.dumps(
            {
                "channel": "pass",
                "backpressure": "pass",
                "output_bytes": len(raw_output),
                "final_stream_offset": expected_offset,
                "resize": resized,
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
