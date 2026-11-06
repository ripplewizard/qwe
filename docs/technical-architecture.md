# QWE Technical Architecture

Status: Implementation Active
Updated: 2026-11-05

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
  |     +-- qwe-session-exec -> systemd scope (Linux) / task process group
  |     +-- bounded output journal
  |
  +-- session worker
        +-- ...
```

The supervisor is replaceable. Session workers are the only owners of live
terminal and process resources. A client never owns a PTY or process group.

The Attach data plane is:

```text
Client -> qwed Attach socket -> worker Attach socket
```

These are dedicated, owner-only sockets separate from request/response control
IPC. The worker remains the authority for live output, input, resize, and VT
state.

This ownership boundary is exercised by `just t12-host-smoke`.

[ADR 0011](adr/0011-consolidate-control-plane.md) is implemented. qwed is the
single control-plane supervisor and directly starts, controls, and recovers
workers. A `flock(2)` runtime lock provides process-lifetime single-instance
protection. Host epoch registration and inventory polling have been removed.

Current production implementation includes durable session workers, SQLite
Task/Lease state, startup reconciliation, and concurrent Attach. Worker
rediscovery validates the expected socket path, Session ID, PID, and start
token. The detailed current protocol and authority boundary are in
[IPC Protocol](ipc-protocol.md).

Production resize and complete structured VT-mode snapshots are implemented on
the long-lived Attach path. Stage 3 adds detached holds, durable attachment
authority, restart reclaim grace, and durable idempotency. Remaining ownership
and liveness matrix work is tracked in
[Development Readiness](development-readiness.md).

## 2. Process Responsibilities

### qwe CLI

- Validates user input.
- Calls `qwed` through local IPC.
- Renders JSON, terminal output, or TUI state.
- Drives the long-lived Attach channel with raw terminal mode, terminal-size
  polling, and `Ctrl-]` detach.
- Never accesses SQLite directly.

### qwed

- Provides the stable public local API.
- Authorizes clients and manages leases.
- Stores metadata, topology, and task history in SQLite.
- Reconciles persisted state against worker inventory before accepting clients.
- Starts and re-adopts workers, observes `SIGCHLD`/`waitpid` events and
  persistent control EOF, and runs the bounded safety scan.
- Authenticates and proxies the dedicated Attach socket directly to workers.
- Does not own PTYs or process groups.

### qwe-worker

- One process per Session; owns the PTY, process group, VT state, and journal.
- Starts the session process through the internal `qwe-session-exec` launcher.
- On Linux, starts `qwe-session-exec` inside a transient systemd user scope.
  The launcher reports the final task PID and cgroup path before exec.
- Serves a persistent request/response control channel and a typed Attach
  channel over owner-only Unix sockets.
- Survives qwed crashes; qwed re-adopts it by socket path, SessionId, worker PID,
  and start token.

### Session worker

The `qwe-worker` process is the Session worker. It calls `setsid`, owns one PTY,
process group, VT state, and bounded journal, and never transfers those
resources to qwed.

## 3. Crate Graph

```text
qwe (CLI)
  -> qwe-protocol
  -> qwe-core

qwed
  -> qwe-core
  -> qwe-protocol
  -> qwe-runtime
  -> qwe-storage

qwe-worker
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
session.kill
session.resource_update
task.status
task.stop
task.kill_batch
task.history
lease.claim
lease.touch
lease.release
lease.takeover
server.kill
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

## 5. Attach and Worker Control Protocols

The accepted V0 Attach endpoint is a dedicated, owner-only Unix socket with
typed binary framing. Its frame numbers, payload limits, payload schemas, and
stable test IDs are defined by the
[canonical protocol schema](../protocol/schema/qwe-v1.schema.json).

The internal worker control protocol is persistent and versioned separately
from client IPC. Its request/response field contract is
[IPC Protocol](ipc-protocol.md):

```text
PING
WORKER_HELLO
SESSION_STATUS
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

The journal is one bounded buffer per Session worker. Each Attach gets a
private 1 MiB output queue; a slow client does not hold the PTY reader or
another client's writer. The journal tail may begin mid-escape-sequence and is
therefore diagnostic data, while the VT screen snapshot is authoritative.

In the V0 implementation, `SESSION_STATUS`, `SESSION_CREATE`, `SESSION_INPUT`,
and `TASK_STOP` return lightweight metadata only. `SESSION_ATTACH` is the
only operation that returns the VT screen and journal tail, preventing status
polling from serializing large responses ahead of PTY input.

`SESSION_INPUT`, `TASK_STOP`, and `SESSION_CLOSE` require attachment authority.
The CLI temporarily attaches for scripted input and accepts a logical owner
plus optional Held generation for teardown.

[ADR 0012](adr/0012-terminal-attach-data-plane.md) is implemented for the V0
exclusive mode. The long-lived channel handles Attach, input, resize, immediate
detach, and connection EOF. The `SESSION_ATTACH` request remains only as a
diagnostic snapshot exposed by `qwe session snapshot`; it is not the interactive
data plane.

The worker registers the subscriber before capturing the snapshot. The
snapshot's `next_stream_offset` is the first offset that may appear in an
`OUTPUT` frame. A per-attachment queue overflows at 1 MiB and closes only that
channel with `output_backpressure`.

Stage 3 moves attachment authority to qwed. qwed validates `ATTACH_OPEN`,
persists the active owner and SHA-256 reclaim-token hash, and only then sends
`ATTACH_ACCEPTED`. It intercepts `DETACH` to create a positive-TTL hold or an
immediate release. Reattach consumes the current Lease generation and writes a
release tombstone. Persisted Active authority becomes Reclaim Grace at qwed
startup; only the matching token can resume the Session during that window.

## 6. State Ownership

| State | Owner | Durable Storage |
|---|---|---|
| Live PTY and writer | Session worker | No |
| VT screen and cursor | Session worker | No |
| Recent terminal bytes | Session worker | No |
| Task process group | Session worker | Identity mirrored to SQLite |
| Session/Pane/TaskInstance metadata | `qwed` | SQLite |
| Lease owner/deadline | `qwed` | SQLite |
| Attachment authority | `qwed` | SQLite token hash, owner, generation, deadline |
| Task history | `qwed` | SQLite |
| Worker control, health, and inventory | `qwed` supervisor | Persistent channels, rebuilt by reconciliation |
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

On restart, qwed scans its runtime directory and reconnects workers after
validating the expected socket path, SessionId, worker PID, and start token.
Recovery never reconstructs a terminal by replaying an unbounded byte stream.

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
- Mutating requests require idempotency keys. qwed uses the logical operation
  and canonical payload as the request fingerprint, replays stored results for
  24 hours, and rejects key reuse for a different request.
- Idempotency reserves a pending record before the effect and marks it complete
  with the replayable response afterward. A pending `SESSION_CREATE` recovers
  a stable response from persisted objects and worker status after restart.
- Pending non-creation mutations return `idempotency_indeterminate` rather than
  re-executing an effect whose outcome cannot be proven.
- Terminal Session, Pane, and Task history is pruned only when both older than
  30 days and outside the newest 10,000 terminal Sessions. Sessions with
  attachment authority are retained.
- Startup reconciliation marks missing running or exited Sessions as `lost`;
  an exited Task remains `exited`.
- Startup probes runtime worker sockets before accepting clients. Orphan health
  is reported, duplicate/identity/closed-session conflicts choose no worker,
  and conflicting socket families are quarantined without deleting them.
- `CONTROL_STATUS.reconciliation` exposes structured
  `orphaned_runtime_entries` and `conflict_runtime_entries`.
- Lifecycle events identify vanished workers; the safety scan covers missed
  events without treating an unresponsive but live worker as exited.
- Workers are spawned as qwed children. `SIGCHLD`/`waitpid` is the primary exit
  path; persistent control EOF marks health unreachable.
- A two-second safety scan probes workers with a 500 ms deadline, without
  holding a global supervisor lock across worker calls.
- `CONTROL_STATUS.reconciliation` is computed from current Sessions, Tasks, and
  worker health. `last_scan_ms` reports the latest completed safety scan rather
  than a static startup value.
- SQLite v10 enforces Session references and stores attachment authority,
  two-phase idempotency replay records, and optional Pane resource policy.
  `CONTROL_STATUS` returns Pane records alongside Sessions and Tasks.
- Lease release writes a generation tombstone; expiration does not kill Tasks.

## 10. Resource Model

- Each Pane/Session has a tree-scoped policy for memory high/max/swap,
  processes-plus-threads, CPU bandwidth, and cumulative CPU time.
- Linux creates one transient systemd user scope per Session. `memory.max`,
  `memory.swap.max`, `pids.max`, and `cpu.max` are hard limits; cumulative CPU
  time is monitored. `memory.oom.group=1` treats the Session as one OOM unit.
- macOS traverses the task tree with `proc_listchildpids` and samples
  `proc_pid_rusage` every 100 ms. Memory, task-count, and CPU limits are
  `monitored`; a breach terminates the process group.
- Status reports requested/effective policy, current and peak memory/task
  usage, cumulative CPU time, enforcement tier, and last violation.
- Dynamic updates replace selected fields on a running Active/Held Session.
  Raising limits is immediate. Lowering a Linux hard memory limit can reclaim
  or OOM immediately; lowering `pids.max` does not kill existing tasks but
  blocks new forks until usage falls below the limit.
- V0 does not claim hard tree limits when systemd user scopes are unavailable.

GPU-to-task attribution remains outside the Pane resource contract and is
reported only when a reliable platform identifier exists.

## 11. Security Baseline

- One local OS user for V0.
- One control client per Session.
- No TCP listener.
- Socket mode `0600`.
- Peer-UID verification.
- Frame and input size limits.
- Secrets are never included in configuration or test evidence.
- API Key Manager uses private credential sources and zeroizing memory. A
  one-shot owner-only broker socket delivers the secret to the internal
  launcher, which re-exposes it to the final child as inherited descriptor 3
  and sets only `QWE_CREDENTIAL_FD=3`.
- Credential files must be private regular files of at most 16 KiB. Missing,
  insecure, symlinked, or oversized sources fail closed.
- Control input is limited to 1 MiB. Client request frames remain limited to
  8 MiB before payload allocation.

## 12. Concurrency and Latency

- `qwed` and workers accept each client connection independently.
- Snapshot locks cover state capture, not socket writes to slow clients.
- The automated gate runs 64 Attach requests with eight clients during
  continuous PTY output.
- V0 requires zero request failures, P95 at or below 3,000 ms, and P99 at or
  below 5,000 ms.

## 13. Product Capabilities

- D3 API Key Manager is implemented through a broker and final-child inherited
  descriptor. Secrets cannot enter args, environment, workspace files, logs,
  or persisted state.
- D3 Pane policy uses Linux cgroup v2 hard limits and macOS monitored libproc
  enforcement. Dynamic updates are supported through
  `SESSION_RESOURCE_UPDATE`. See ADR 0018.
- D3 forced teardown implements `kill-session`, confirmed `kill-server`, and
  batch Task termination. See ADR 0017.

## 14. Open Decisions

The Stage 0 design gate and automated Stage 1-3 acceptance are closed. These
decisions remain open and must be recorded in ADRs before implementation:

1. Whether read-only observers are supported beyond the reserved Attach mode.
2. Final VT engine after broader compatibility benchmarks.
3. Exact idle-timeout defaults for non-terminal Sessions.

These decisions must be recorded as accepted ADRs before their implementation.
