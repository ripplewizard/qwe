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


def send_control(path, operation, payload={}, key=None):
    request = {
        "version": PROTOCOL_VERSION,
        "request_id": f"attach-probe-{time.monotonic_ns()}",
        "idempotency_key": key,
        "operation": operation,
        "payload": payload,
    }
    encoded = json.dumps(request, separators=(",", ":")).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(5)
        stream.connect(path)
        stream.sendall(struct.pack(">I", len(encoded)))
        stream.sendall(encoded)
        length = struct.unpack(">I", receive_exact(stream, 4))[0]
        response = json.loads(receive_exact(stream, length))
    result = response["result"]
    return result.get("Ok", result.get("Err"))


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


def open_frame(
    session_id,
    owner,
    mode="exclusive",
    version=PROTOCOL_VERSION,
    reclaim_token=None,
    lease_generation=None,
):
    frame = {
        "protocol_version": version,
        "session_id": session_id,
        "owner": owner,
        "rows": 24,
        "columns": 80,
        "mode": mode,
    }
    if reclaim_token is not None:
        frame["reclaim_token"] = reclaim_token
    if lease_generation is not None:
        frame["lease_generation"] = lease_generation
    return frame


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
    assert len(accepted["reclaim_token"]) == 64

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

    active_claim = send_control(
        args.socket,
        "LEASE_CLAIM",
        {"session_id": args.session_id, "owner": "other-owner", "ttl_ms": 30000},
        key="active-lease-claim",
    )
    assert active_claim["code"] == "already_attached", active_claim

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

    held_owner = "hold-probe"
    holding = connect(attach_path)
    send_json(holding, ATTACH_OPEN, open_frame(args.session_id, held_owner))
    frame_type, _ = receive_frame(holding)
    assert frame_type == ATTACH_ACCEPTED
    frame_type, _ = receive_frame(holding)
    assert frame_type == SNAPSHOT
    send_json(
        holding,
        DETACH,
        {"protocol_version": PROTOCOL_VERSION, "hold_ttl_ms": 20000},
    )
    expect_eof(holding)
    holding.close()

    held_lease = send_control(
        args.socket,
        "LEASE_STATUS",
        {"session_id": args.session_id},
    )
    assert held_lease["owner"] == held_owner, held_lease
    assert held_lease["generation"] == 1, held_lease
    assert held_lease["state"] == "active", held_lease
    touched_lease = send_control(
        args.socket,
        "LEASE_TOUCH",
        {
            "session_id": args.session_id,
            "owner": held_owner,
            "generation": 1,
            "ttl_ms": 30000,
        },
        key="touch-detached-hold",
    )
    assert touched_lease["generation"] == 1, touched_lease
    assert touched_lease["deadline_ms"] >= held_lease["deadline_ms"], touched_lease

    wrong_owner = connect(attach_path)
    send_json(
        wrong_owner,
        ATTACH_OPEN,
        open_frame(args.session_id, "other-owner", lease_generation=1),
    )
    expect_error(wrong_owner, "not_owner")

    wrong_generation = connect(attach_path)
    send_json(
        wrong_generation,
        ATTACH_OPEN,
        open_frame(args.session_id, held_owner, lease_generation=0),
    )
    expect_error(wrong_generation, "not_owner")

    reclaimed = connect(attach_path)
    send_json(
        reclaimed,
        ATTACH_OPEN,
        open_frame(args.session_id, held_owner, lease_generation=1),
    )
    frame_type, payload = receive_frame(reclaimed)
    assert frame_type == ATTACH_ACCEPTED
    reclaimed_accepted = json.loads(payload)
    assert reclaimed_accepted["generation"] == 2, reclaimed_accepted
    frame_type, _ = receive_frame(reclaimed)
    assert frame_type == SNAPSHOT
    send_payload(reclaimed, INPUT, b"printf 'HOLD_REATTACH\\n'\r")
    deadline = time.monotonic() + 5
    held_output = bytearray()
    while b"HOLD_REATTACH" not in held_output and time.monotonic() < deadline:
        frame_type, payload = receive_frame(reclaimed)
        if frame_type == OUTPUT:
            held_output.extend(payload[8:])
    assert b"HOLD_REATTACH" in held_output
    released_lease = send_control(
        args.socket,
        "LEASE_STATUS",
        {"session_id": args.session_id},
    )
    assert released_lease["generation"] == 2, released_lease
    assert released_lease["state"] == "released", released_lease
    assert released_lease["released_at_ms"] is not None, released_lease
    send_json(
        reclaimed,
        DETACH,
        {"protocol_version": PROTOCOL_VERSION, "hold_ttl_ms": 0},
    )
    expect_eof(reclaimed)
    reclaimed.close()

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
                "detached_hold": "pass",
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
