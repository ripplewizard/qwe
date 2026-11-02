# Session Attach/Detach POC Contract

Status: automated continuity implemented and passing for Rust, Go, and C++;
manual interactive screen review remains.

T08 validates session state continuity only. Full-screen interactive UX and
production attach streaming are outside this slice.

## Operations

- `SESSION_START <request-id> <session-id> <output-log> <program> [args...]`
  creates a PTY-backed session.
- `SESSION_ATTACH <request-id> <session-id> <client-id>` requires the client to
  own the Session Lease and returns a replayable terminal snapshot.
- `SESSION_DETACH <request-id> <session-id> <client-id>` records that the owner
  is disconnected without terminating the PTY or releasing the lease.
- `SESSION_INPUT <request-id> <session-id> <client-id> <hex-bytes>` is accepted
  only while the owner is attached; it writes to the PTY and refreshes the
  lease.
- `SESSION_STATUS <request-id> <session-id>` reports whether the PTY process is
  running, whether an owner is attached, output size, and a replayable snapshot.

Successful session responses include `session_id`, `pid`, `running`,
`attached`, `output_bytes`, and `snapshot_hex`.

The snapshot is the complete raw PTY output stream in hexadecimal form.
Replaying it reconstructs the current terminal state with the implementation's
VT parser. This is intentionally transport-neutral for the POC.

## Continuity Requirements

After detach:

1. The PTY remains open.
2. The shell process remains alive.
3. Program output produced after detach is retained.
4. The lease owner remains unchanged until timeout.

After reattach by the same owner:

1. The full accumulated output can be replayed.
2. New input is accepted.
3. New output appears in the session snapshot.
