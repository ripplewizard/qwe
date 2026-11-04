# QWE Technical Architecture

Status: Implementation Active
Updated: 2026-11-04

## 1. Process Model

```text
Client
  |
  | versioned client IPC
  v
qwed supervisor ------------ SQLite WAL
  |
  +-- session worker
  |     +-- PTY and VT state
  |     +-- task process group
  |     +-- bounded output journal
  |
  +-- session worker
        +-- ...
```

The supervisor is replaceable. Session workers are the only owners of live
terminal and process resources. A client never owns a PTY or process group.

This ownership boundary is exercised by `just t12-host-smoke`.

[ADR 0011](adr/0011-consolidate-control-plane.md) accepts one qwed supervisor
for V0. The current code still uses the migration topology
`qwed -> qwe-host -> session worker`, including Host epoch recovery and 250 ms
inventory polling. The Stage 1 cutover moves worker inventory and lifecycle
events into qwed without changing the worker isolation boundary.

Current production implementation includes durable session workers, SQLite
Task/Lease state, startup reconciliation, and concurrent Attach. Worker
rediscovery validates the expected socket path, Session ID, PID, and start
token. The detailed current protocol and authority boundary are in
[IPC Protocol](ipc-protocol.md).

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
- Reconciles persisted state against worker inventory before accepting clients.
- Starts workers, observes lifecycle events, and runs the bounded safety scan.
- Does not own PTYs or process groups.

### qwe-host

- Temporary migration component that manages isolated session workers until the
  Stage 1 supervisor cutover.
- Owns Host epoch registration and the current 250 ms inventory path.
- Is removed from the V0 process model after migration.

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

## 5. Attach Protocol and Migration Host Protocol

The accepted V0 Attach endpoint is a dedicated, owner-only Unix socket with
typed binary framing. Its frame numbers, payload limits, payload schemas, and
stable test IDs are defined by the
[canonical protocol schema](../protocol/schema/qwe-v1.schema.json).

The current migration Host protocol is internal and versioned separately from
client IPC. The implemented field-level contract is
[IPC Protocol](ipc-protocol.md):

```text
HOST_REGISTER
HOST_LIST
PING
SESSION_CREATE
SESSION_ATTACH
SESSION_INPUT
TASK_STOP
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
and `TASK_STOP` return lightweight metadata only. `SESSION_ATTACH` is the
only operation that returns the VT screen and journal tail, preventing status
polling from serializing large responses ahead of PTY input.

The current `session detach` CLI command returns `unsupported_operation` until
the Stage 2 channel provides an actual attachment-state transition.

[ADR 0012](adr/0012-terminal-attach-data-plane.md) is accepted. Stage 2 replaces
one-shot Attach, input, resize, and detach calls with the long-lived channel.
The current request/response table remains the migration contract until that
implementation lands.

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
| Worker health/inventory | `qwed` supervisor | Runtime, rebuilt by reconciliation |
| Configuration | Workspace files | Files |

## 7. Recovery Semantics

| Failure | Expected Behavior |
|---|---|
| CLI disconnect | Session and task continue; attach later |
| `qwed` SIGKILL | All session workers continue |
| `qwed` restart | Rediscover workers and reconcile SQLite inventory |
| Corrupt SQLite | Fail explicitly; do not start with silent data loss |
| Session worker SIGKILL | That Session is lost; return `terminal_lost` |
| OS reboot | No V0 recovery |

The current migration topology additionally recovers `qwe-host` by scanning the
runtime directory and reconnecting workers. The accepted target removes Host
from this path while retaining socket discovery, identity validation, and
worker-owned PTY state. Recovery never reconstructs a terminal by replaying an
unbounded byte stream.

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
- Requests carry idempotency keys, but durable deduplication and response
  replay are not implemented until Stage 3.
- Startup reconciliation marks missing running or exited Sessions as `lost`;
  an exited Task remains `exited`.
- Lifecycle events identify vanished workers; the safety scan covers missed
  events without treating an unresponsive but live worker as exited.
- The current migration topology retains the 250 ms inventory poll until the
  Stage 1 cutover.
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

## 14. Accepted Contracts Awaiting Implementation

The Stage 0 design gate is closed. The following accepted decisions are now
implementation work:

1. Consolidate qwed and qwe-host in Stage 1.
2. Implement the long-lived Attach channel in Stage 2.
3. Migrate storage and API behavior to the accepted 1:1 object model.
4. Enforce the accepted attachment authorization matrix.
5. Implement event-first worker liveness and reconciliation.

Other decisions still open and must be recorded in ADRs before implementation:

1. Whether read-only observers are supported beyond the reserved Attach mode.
2. Final VT engine after broader compatibility benchmarks.
3. Lease clock-anomaly behavior and default detached-hold TTL before D2.
4. Idempotency key scope, response replay, and retention before D2.
5. Session/Task history retention policy before V0.
6. Exact idle-timeout defaults for non-terminal Sessions.

These decisions must be recorded as accepted ADRs before their implementation.
