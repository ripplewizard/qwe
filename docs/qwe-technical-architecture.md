# QWE Technical Architecture

Status: Implementation Draft
Version: 0.1
Date: 2026-11-03

## 1. Process Model

```text
Client
  |
  | versioned client IPC
  v
qwed control daemon -------- SQLite WAL
  |
  | versioned host IPC
  v
qwe-host
  +-- terminal sessions
  +-- PTYs and VT state
  +-- task process groups
  +-- bounded output journals
```

The daemon is replaceable. The host is the only owner of live terminal and
process resources. A client never owns a PTY or process group.

This ownership boundary is exercised by `just t12-host-smoke`.

## 2. Process Responsibilities

### qwe CLI

- Validates user input.
- Calls `qwed` through local IPC.
- Renders JSON, terminal output, or TUI state.
- Never accesses SQLite directly.

### qwed

- Provides the stable public local API.
- Authorizes clients and manages leases.
- Stores metadata, topology, and task history in SQLite.
- Starts or reconnects to `qwe-host`.
- Does not own PTYs or process groups.

### qwe-host

- Owns PTYs, terminal emulator state, input streams, and process groups.
- Enforces bounded output retention and backpressure.
- Exposes session and task operations over the host protocol.
- Remains alive when `qwed` is killed or restarted.

## 3. Crate Graph

```text
qwe (CLI)
  -> qwe-protocol
  -> qwe-core

qwed
  -> qwe-core
  -> qwe-protocol
  -> qwe-storage

qwe-host
  -> qwe-core
  -> qwe-protocol
  -> qwe-runtime

qwe-runtime
  -> qwe-core

qwe-storage
  -> qwe-core
```

Dependencies point inward. `qwe-core` contains identifiers, object semantics,
states, and invariant errors, but no PTY, socket, SQLite, or CLI code.

## 4. Client IPC

- Unix Domain Socket only.
- Length-prefixed JSON.
- Mandatory protocol version.
- Required request ID.
- Required idempotency key for mutating operations.
- Owner-only socket permissions.
- Same-UID peer credential check.

Operations:

```text
session.create
session.attach
session.detach
session.input
session.resize
session.status
session.close
task.start
task.status
task.stop
task.history
lease.claim
lease.touch
lease.release
lease.takeover
```

Errors are stable codes, for example:

```text
incompatible_version
unauthorized_peer
not_found
not_owner
not_attached
already_exists
truncated_history
persistence_failure
```

## 5. Host Protocol

The host protocol is internal and versioned separately from client IPC:

```text
HOST_HELLO
SESSION_CREATE
SESSION_ATTACH
SESSION_DETACH
SESSION_INPUT
SESSION_RESIZE
SESSION_SNAPSHOT
SESSION_CLOSE
TASK_START
TASK_STATUS
TASK_STOP
HOST_SHUTDOWN
```

`SESSION_SNAPSHOT` returns:

- Session ID and terminal dimensions.
- Cursor position, cursor visibility, and terminal modes.
- Visible screen and bounded scrollback.
- Bounded recent byte journal.
- `truncated_from` sequence number.

It does not replay an unbounded historical PTY stream.

In the V0 implementation, `SESSION_STATUS`, `SESSION_CREATE`, `SESSION_INPUT`,
and `SESSION_DETACH` return lightweight metadata only. `SESSION_ATTACH` is the
only operation that returns the VT screen and journal tail, preventing status
polling from serializing large responses ahead of PTY input.

## 6. State Ownership

| State | Owner | Durable Storage |
|---|---|---|
| Live PTY and writer | `qwe-host` | No |
| VT screen and cursor | `qwe-host` | Periodic/offline snapshot |
| Recent terminal bytes | `qwe-host` | Optional checkpoint |
| Task process group | `qwe-host` | Identity mirrored to SQLite |
| Session metadata | `qwe-host`, mirrored by `qwed` | SQLite |
| Lease owner/deadline | `qwed` | SQLite |
| Task history | `qwed` | SQLite |
| Configuration | Workspace files | Files |

## 7. Recovery Semantics

| Failure | Expected Behavior |
|---|---|
| CLI disconnect | Session and task continue; attach later |
| `qwed` SIGKILL | `qwe-host` and all live resources continue |
| `qwed` restart | Rediscover host and reconstruct control state |
| Corrupt SQLite | Fail explicitly; do not start with silent data loss |
| `qwe-host` SIGKILL | Sessions are lost; return `terminal_lost` |
| OS reboot | No V0 recovery |

If host restart recovery is later required, it must use compact VT snapshots
and process identity, not full terminal replay.

## 8. Task Identity

An opaque UUIDv7 `TaskId` is the externally visible identity. A local process
identity includes:

- PID and PGID.
- PID start token.
- Node ID.
- Linux boot ID where applicable.

The process start token is mandatory. PID alone is never sufficient.

## 9. Persistence

- SQLite runs in WAL mode.
- Migrations are monotonic and transactional.
- A newer schema is rejected by older binaries.
- High-frequency terminal bytes are not written to SQLite per frame.
- Mutating requests retain idempotency keys to prevent duplicate tasks.

## 10. Resource Model

V0 reports:

- Process CPU and process-group CPU.
- RSS, process-tree RSS, and an explicit warning about shared pages.
- Sampling latency and monitor overhead.
- GPU capability and device metrics where supported.

GPU-to-task attribution is returned only when a reliable platform identifier
exists; otherwise the field is `unsupported`.

## 11. Security Baseline

- One local OS user for V0.
- One control client per Session.
- No TCP listener.
- Socket mode `0600`.
- Peer-UID verification.
- Frame and input size limits.
- Secrets are never included in configuration or test evidence.

## 12. Remaining Architecture Decisions

1. Whether host crash recovery is required before V1.
2. Whether read-only observers are supported.
3. Final VT engine after bounded snapshot benchmarks.
4. Exact idle-timeout defaults for non-terminal Sessions.

These decisions must be recorded as ADRs before their implementation.
