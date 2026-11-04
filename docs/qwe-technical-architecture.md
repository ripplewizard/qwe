# QWE Technical Architecture

Status: Implementation Active
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
qwe-host (recoverable manager)
  |
  +-- session worker
  |     +-- PTY and VT state
  |     +-- task process group
  |     +-- bounded output journal
  |
  +-- session worker
        +-- ...
```

The daemon and Host manager are replaceable. Session workers are the only
owners of live terminal and process resources. A client never owns a PTY or
process group.

This ownership boundary is exercised by `just t12-host-smoke`.

Current production implementation includes durable session workers, Host epoch
recovery, SQLite Task/Lease state, startup reconciliation, and concurrent
Attach. `qwed` also reconciles Host inventory every 250 ms. Worker rediscovery
validates the expected socket path and worker start token. The detailed
protocol and authority boundary are in [IPC Protocol](ipc-protocol.md).

Production resize and complete VT-mode snapshots remain in progress; see
[Development Readiness](development-readiness.md).

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
- Reconciles persisted state against Host inventory before accepting clients.
- Starts or reconnects to `qwe-host`.
- Does not own PTYs or process groups.

### qwe-host

- Manages isolated session workers and reconnects to them after Host restart.
- Enforces bounded output retention and backpressure.
- Exposes session and task operations over the host protocol.
- Remains alive when `qwed` is killed or restarted.

### Session worker

- Calls `setsid` and owns one PTY, process group, VT state, and bounded journal.
- Serves status, Attach, input, and close over an owner-only Unix socket.
- Survives `qwed` and Host crashes; worker failure loses only that Session.

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

The host protocol is internal and versioned separately from client IPC. The
authoritative field-level contract is [IPC Protocol](ipc-protocol.md):

```text
HOST_REGISTER
HOST_LIST
PING
SESSION_CREATE
SESSION_ATTACH
SESSION_DETACH
SESSION_INPUT
SESSION_CLOSE
```

`SESSION_SNAPSHOT` returns:

- Session ID and terminal dimensions.
- Cursor position, cursor visibility, and terminal modes.
- Visible screen and bounded scrollback.
- Bounded recent byte journal.
- `truncated_from` sequence number.

It does not replay an unbounded historical PTY stream.

The journal is one bounded buffer per Session worker. Each concurrent Attach
gets a private snapshot; a slow client does not hold the PTY reader or another
client's response writer. The journal tail may begin mid-escape-sequence and is
therefore diagnostic data, while the VT screen snapshot is authoritative.

In the V0 implementation, `SESSION_STATUS`, `SESSION_CREATE`, `SESSION_INPUT`,
and `SESSION_DETACH` return lightweight metadata only. `SESSION_ATTACH` is the
only operation that returns the VT screen and journal tail, preventing status
polling from serializing large responses ahead of PTY input.

## 6. State Ownership

| State | Owner | Durable Storage |
|---|---|---|
| Live PTY and writer | Session worker | No |
| VT screen and cursor | Session worker | No |
| Recent terminal bytes | Session worker | No |
| Task process group | Session worker | Identity mirrored to SQLite |
| Session metadata | `qwed` | SQLite |
| Lease owner/deadline | `qwed` | SQLite |
| Task history | `qwed` | SQLite |
| Configuration | Workspace files | Files |

## 7. Recovery Semantics

| Failure | Expected Behavior |
|---|---|
| CLI disconnect | Session and task continue; attach later |
| `qwed` SIGKILL | Host and all session workers continue |
| `qwed` restart | Rediscover Host and reconcile SQLite inventory |
| Corrupt SQLite | Fail explicitly; do not start with silent data loss |
| `qwe-host` SIGKILL | Restart scans runtime directory and reconnects workers |
| Session worker SIGKILL | That Session is lost; return `terminal_lost` |
| OS reboot | No V0 recovery |

Host recovery uses socket discovery and worker-owned PTY state. It never
reconstructs a terminal by replaying an unbounded byte stream.

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
- Startup reconciliation marks missing Sessions and Tasks as `lost`.
- Runtime reconciliation marks a vanished worker `lost` within 250 ms.
- Lease release writes a generation tombstone; expiration does not kill Tasks.

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

## 12. Concurrency and Latency

- `qwed`, Host, and workers accept each client connection independently.
- Snapshot locks cover state capture, not socket writes to slow clients.
- The automated gate runs 64 Attach requests with eight clients during
  continuous PTY output.
- V0 requires zero request failures, P95 at or below 3,000 ms, and P99 at or
  below 5,000 ms.

## 13. Product Capabilities Scheduled

- D3 API Key Manager uses a broker or inherited descriptor; secrets cannot
  enter args, environment, files, logs, or persisted state.
- D3 Pane resource policy covers CPU, memory, and process limits.
- D3 forced teardown covers Session, Server, and batch Task termination.

## 14. Remaining Architecture Decisions

1. Whether read-only observers are supported.
2. Final VT engine after broader compatibility benchmarks.
3. Exact idle-timeout defaults for non-terminal Sessions.

These decisions must be recorded as ADRs before their implementation.
