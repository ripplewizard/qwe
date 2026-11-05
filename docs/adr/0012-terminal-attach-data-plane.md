# ADR 0012: Terminal Attach Data Plane

Status: Accepted
Date: 2026-11-04
Implementation state: Stage 2 channel implemented. Stage 3 now persists active
authority, detached holds, Lease release tombstones, and supervisor-restart
reclaim grace; full control-operation authorization remains in progress.

## Context

The control IPC is request/response: one connection carries one request and one
response. `SESSION_ATTACH` currently returns a bounded snapshot and then closes.
It does not carry subsequent terminal output.

ADR 0011 binds active attachment ownership to connection lifetime and requires
interactive Attach. Those requirements cannot be met through a one-shot
request. A terminal channel must remain open while output, input, resize, and
detach events flow in both directions.

Raw PTY bytes must not be base64-encoded or JSON-escaped for live output. Control
messages and terminal bytes therefore need distinct typed frames.

## Decision Drivers

- Make snapshot-to-live handoff lossless and duplicate-free.
- Avoid encoding overhead on the PTY output hot path.
- Keep ownership tied to one identifiable connection.
- Keep control IPC simple and backward-readable during pre-release development.
- Reserve extension points without claiming unsupported observer behavior.

## Decision

Add a dedicated, owner-only, long-lived terminal attach endpoint beside the
request/response control endpoint. Both listen under the private runtime
directory and verify the same OS UID.

The attach endpoint uses typed binary framing:

```text
frame_type: u8 | payload_length: u32 big-endian | payload
```

The attach handshake negotiates the channel protocol version. Unknown frame
types or versions close the channel with an explicit protocol error.

### Frame Types

Client-to-server frames:

| Type | Payload |
|---|---|
| `ATTACH_OPEN` | Versioned JSON attach request |
| `INPUT` | Raw terminal input bytes |
| `RESIZE` | Versioned JSON rows and columns |
| `ACK` | Versioned JSON acknowledged stream offset |
| `DETACH` | Versioned JSON requested hold TTL or immediate release |

Server-to-client frames:

| Type | Payload |
|---|---|
| `ATTACH_ACCEPTED` | Versioned JSON ownership and terminal metadata |
| `SNAPSHOT` | Versioned structured terminal snapshot |
| `OUTPUT` | `stream_offset: u64 big-endian` followed by raw PTY bytes |
| `RESIZED` | Versioned JSON applied geometry and stream offset |
| `PROCESS_EXITED` | Versioned JSON exit code, signal, and stream offset |
| `OWNERSHIP_REVOKED` | Versioned JSON reason |
| `ERROR` | Stable error code and message |

Control and lifecycle payloads use JSON. `INPUT` and live PTY bytes use raw
payloads. Each `OUTPUT` frame is at most 64 KiB.

### Attach Handshake

`ATTACH_OPEN` contains the Session ID, logical owner, optional reclaim token,
optional detached-hold Lease generation, terminal dimensions, and attachment
mode:

```text
mode = exclusive | read_only
```

The field defaults to `exclusive` when omitted. V0 supports only `exclusive`.
`read_only` is reserved so observer support can be added without a second
framing protocol; until an observer ADR is accepted, `read_only` returns
`unsupported_mode`.

Every successful attach returns a fresh CSPRNG reclaim token. The raw token is
held only by the client; the previous token becomes invalid. Persistence and
comparison follow ADR 0011.

When the Session is in a detached hold, the owner must provide the current
Lease generation. A successful Attach consumes the hold and writes a release
tombstone before the channel becomes Active.

### Snapshot and Live Handoff

The worker registers the attachment as a subscriber before capturing the
snapshot. The snapshot records `next_stream_offset = N`; every later `OUTPUT`
frame starts exactly at the client's expected offset and advances it by the
number of raw bytes in the frame.

The server sends `SNAPSHOT` before any `OUTPUT` frame from offset `N`. A client
must never receive an output gap or duplicate on a healthy channel. If the
server cannot preserve that guarantee, it closes the channel and the client
obtains a fresh snapshot.

Resize and process-exit frames include the stream offset at which their state
became effective.

### Backpressure

Each attachment has an independent output queue capped at 1 MiB. A slow channel
never blocks the PTY reader or another attachment. When the queue overflows,
qwed closes that channel with `output_backpressure`; the client may reconnect
and receive a fresh snapshot. Silent output dropping is prohibited.

`ACK` reports the highest contiguous stream offset applied by the client. It is
used for progress diagnostics and stuck-client detection; it does not delete or
advance the worker journal. The queue limit measures outbound data not yet
written to the socket.

### Concurrency

The eight-client latency benchmark uses eight independent Sessions. Contention
for one Session is a separate correctness test and must produce the
authorization result defined by ADR 0014, not a latency sample.

## Compatibility

This is a new pre-release channel contract. The control endpoint remains
length-prefixed JSON. `session.attach`, `session.input`, `session.resize`, and
`session.detach` are no longer one-shot control operations after migration.

## Alternatives Considered

### Keep Attach as One-Shot Requests

Rejected. It cannot deliver live TUI output or support connection-bound active
ownership.

### JSON or Base64 for PTY Output

Rejected for the output hot path. It increases payload size and error surface
without a compatibility benefit.

### Reuse the Request/Response Operation Field

Rejected. A bidirectional asynchronous channel needs explicit frame directions,
server-initiated frames, and independent flow control.

## Consequences

- Interactive Attach has a stable transport independent of control IPC.
- Snapshot continuity becomes a testable invariant.
- Existing one-shot Attach code is replaced rather than extended.
- A machine-readable frame schema is required before the protocol freezes.

## Acceptance Required

- Approve a dedicated long-lived attach endpoint.
- Approve typed binary framing and raw PTY output frames.
- Approve the `mode` field with only `exclusive` supported in V0.
- Approve the 1 MiB per-attachment queue and close-on-overflow policy.
