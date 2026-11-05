# QWE IPC Protocol

Status: Stages 1-3 implemented and validated on macOS and Linux
Date: 2026-11-05

This document describes the currently implemented request/response protocol.
The accepted V0 Attach frames and state contracts are canonical in
[`protocol/schema/qwe-v1.schema.json`](../protocol/schema/qwe-v1.schema.json).
ADR [0011](adr/0011-consolidate-control-plane.md) defines the supervisor and
persistent worker-control model. ADRs
[0012](adr/0012-terminal-attach-data-plane.md) and
[0013](adr/0013-core-object-model.md) define the Attach and object contracts.

## 1. Authority Boundaries

- `qwed` is the sole authority for SQLite Sessions, Tasks, and Leases.
- `qwed` owns live worker inventory, control channels, and health state.
- `qwe-worker` processes own PTYs, process groups, VT state, and journals.
- Clients never connect directly to worker control sockets.
- Clients connect to qwed's dedicated Attach socket. qwed proxies the channel
  to the worker Attach socket without transferring PTY ownership.

## 2. Transport

All links use SOCK_STREAM Unix Domain Sockets. Messages are a four-byte
big-endian length followed by JSON. Frames are limited to 8 MiB. Sockets are
mode `0600`; every server also verifies peer UID.

Request fields:

```json
{
  "version": 1,
  "request_id": "unique-id",
  "idempotency_key": "optional",
  "operation": "SESSION_STATUS",
  "payload": {}
}
```

## 3. Supervisor Lifecycle

`qwed` holds a `flock(2)` runtime lock for its full process lifetime. The lock
file is fixed; PID-file contents are not used for singleton authority.

Workers started by qwed are direct children. Their exits are observed through
`SIGCHLD` and `waitpid`. Every worker has two persistent control connections:
one for normal operations and one for health probes. EOF on a control
connection marks health `unreachable` immediately.

A safety scan runs every two seconds and uses the dedicated health connection
with a 500 ms PING deadline. Process absence or start-token mismatch marks the
Session/Task `lost`; a live but unresponsive worker remains `unreachable`.
Calls to one worker are isolated and never hold a global supervisor lock.

## 4. Runtime Reconciliation

qwed scans its runtime directory before accepting clients. A worker is accepted
only when:

- The socket path matches the expected session-derived path.
- `session_id` matches.
- Worker PID and PID start token match the persisted worker identity.

Unknown runtime workers are reported as orphans and are not adopted. Identity
conflicts are quarantined without resurrecting a closed or mismatched Session.

## 5. Session Worker Protocol

Worker operations are `PING`, `WORKER_HELLO`, `SESSION_STATUS`,
`SESSION_ATTACH`, `SESSION_INPUT`, `TASK_STOP`, and `SESSION_CLOSE`. The
request/response channel is persistent and supports concurrent in-flight
requests keyed by `request_id`. `PING` is a lightweight probe and does not
capture the terminal journal.

`SESSION_INPUT`, `TASK_STOP`, and `SESSION_CLOSE` require the current Active
owner. A Held Session requires the owner and Lease generation. Unattached and
Reclaim Grace Sessions return `not_attached`. The CLI can set the logical owner
with global `--owner` and provides `--generation` on close and task-stop.
Scripted `session input` establishes a temporary Active attachment rather than
bypassing the matrix.

The legacy control operation behind `session detach` still returns
`unsupported_operation`; interactive detach is now the `DETACH` frame on the
long-lived Attach channel. A no-op control success is not accepted.

`SESSION_ATTACH` remains as a diagnostic request/response snapshot. The CLI
exposes it as `session snapshot`. It returns structured terminal state, a
bounded screen snapshot, and at most the newest 1 MiB journal tail. The journal
tail is opaque raw bytes; the screen snapshot is authoritative.

Interactive Attach uses a separate owner-only socket and the accepted typed
binary frame contract. qwed proxies directly to the worker Attach socket; the
first `ATTACH_OPEN` selects the Session. The worker registers the
subscriber before capturing `SNAPSHOT`, so `OUTPUT` starts at
`next_stream_offset` without gaps. `INPUT` and PTY output are raw bytes.
`RESIZE` is ordered against output by a worker stream lock. EOF releases the
Active attachment immediately.

V0 `read_only` is rejected with `unsupported_mode`. Immediate `DETACH` closes
the channel and releases authority. A positive-TTL `DETACH` closes the channel
and creates a Lease-backed Hold. Reattach requires the current owner and Lease
generation and writes a release tombstone before returning Active.

qwed decides `ATTACH_OPEN` before opening the worker data channel. It returns a
256-bit CSPRNG reclaim token only after persisting the active owner and token
hash. The worker receives a separate qwed-owned channel identity. Persisted
Active authority becomes Reclaim Grace on qwed restart; only the matching raw
token may resume during the grace window.

`SESSION_CREATE` is the sole creation operation. It atomically creates one
Session, Pane, and TaskInstance. The response includes `session_id`, `pane_id`,
and `task_id`. `TASK_START` is absent from the public protocol, and the CLI
`task start` command was removed during the Stage 1 clean cutover.

`CONTROL_STATUS` computes reconciliation from current durable and worker state.
It returns `sessions`, `panes`, `tasks`, `workers`, and a `reconciliation`
object containing current lost/unreachable and task terminal counts,
structured orphan/conflict runtime entries, and `last_scan_ms`. Startup probes
worker sockets before accepting clients; ambiguous workers are not adopted.

## 6. Lease State Machine

Leases use UTC wall-clock milliseconds and these states:

- Active: owner and deadline are set.
- Expired: deadline is not in the future; ownership operations require
  claim/takeover.
- Released: owner is empty and `released_at_ms` is set; generation remains as a
  tombstone.

`owner` is an application-level client identity, not a PID. Expiry never kills
a Task or Session. Generation increments on takeover and release, so accepted
operations cannot reuse an older fencing token.

Lease and detached-hold TTLs are positive and at most 24 hours. A renewal uses
`max(now_ms, updated_at_ms) + ttl_ms`, so a backward wall-clock correction does
not shorten the deadline. A forward jump may expire authority but never kills
terminal work. ADR 0016 records the proposed defaults and retention policy.

## 7. Idempotency

Mutating operations require a non-empty `idempotency_key` of at most 256 bytes.
The key is scoped by the logical operation and canonical request payload.

- Repeating the same request returns the persisted result, including after
  qwed restart.
- Reusing the key for a different operation or payload returns
  `idempotency_conflict`.
- Results and deterministic protocol errors are retained for 24 hours.
- Missing keys on mutating operations return `invalid_request`.
- A pending `SESSION_CREATE` is reconstructed from durable Session/Pane/Task
  state and the adopted worker, then completed without creating a duplicate.
- Other interrupted pending mutations return `idempotency_indeterminate` rather
  than automatically repeating a possibly applied effect.

## 8. Backpressure

Each Session has one bounded 16 MiB journal. One-shot snapshot returns at most
1 MiB of diagnostic journal tail. Each live attachment has an independent
1 MiB output queue and closes only that channel on overflow; no silent output
loss is permitted.

## 9. Security Model

Owner-only sockets prevent other UIDs from connecting at the OS layer.
Application peer-UID checks remain defense in depth for misconfigured paths,
future group-accessible sockets, and compromised file permissions.
