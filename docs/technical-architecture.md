# QWE Technical Architecture

Status: Implementation Active
Updated: 2026-11-09

## 1. Process Model

```text
Client
  |
  | versioned client IPC
  v
qwed supervisor ------------ SQLite WAL
  |
  +-- Session
        +-- Pane ResourceScope (Linux systemd user scope)
        |     +-- control cgroup: Pane worker and bounded metadata
        |     +-- workload cgroup: PTY task and fresh-process agent commands
        |
        +-- Pane ResourceScope
              +-- Pane worker
                    +-- ...
```

The diagram shows the implemented multi-Pane topology from
[ADR 0021](adr/0021-shared-prerequisites.md). A Session may own multiple Pane
workers; existing one-Pane Sessions remain valid.

The supervisor is replaceable. Pane workers are the only owners of live
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
- Runs `session compose` as a client-side projection over one Pane-targeted
  Attach channel per displayed Pane.
- Implements Pane-clamped visible-screen copy and a quick menu without
  forwarding menu bytes to PTY input.
- Never accesses SQLite directly.

### qwed

- Provides the stable public local API.
- Authorizes clients and manages leases.
- Stores metadata, topology, and task history in SQLite.
- Authorizes security operations and agent requests against hashed,
  Principal-bound capabilities; reloads ancestor epochs on every request and
  runs the freeze-expiry invigilator.
- Reconciles persisted state against worker inventory before accepting clients.
- Starts and re-adopts Pane workers, observes `SIGCHLD`/`waitpid` events and
  persistent control EOF, and runs the bounded safety scan. Inventory,
  recovery, health, and control routing are keyed by Pane.
- On Linux, starts each Pane worker inside its transient systemd user scope so
  the worker's `control` child and the PTY/command `workload` child remain in
  one Pane ResourceScope.
- Authenticates and proxies the dedicated Attach socket to the selected Pane
  worker. Attach may select an optional Pane; omission preserves the default
  Pane. Observer access is a qwed-owned read-only projection.
- Does not own PTYs or process groups.

### qwe-worker

- One process per Pane. It owns the PTY, process group, VT state, and journal.
- Starts the session process as a child of the worker. In the production
  Linux path, qwed has already placed the worker inside the Pane scope; the
  worker moves its task and agent commands into the delegated `workload`
  cgroup.
- Starts Stages 9.1-9.5 agent commands in the same worker process tree with
  separate stdout/stderr pipes or one merged PTY stream, bounded semantic event
  replay, and streaming credential redaction.
- Reads the PTY foreground process group for interactive shells so
  `wait_idle` can distinguish prompt idle from a foreground child.
- Serves a persistent request/response control channel and a typed Attach
  channel over owner-only Unix sockets.
- Survives qwed crashes; qwed re-adopts it by socket path, SessionId, PaneId,
  worker PID, and start token. PaneId validation is added with schema v11.

### Pane worker

The `qwe-worker` process is a Pane worker. It calls `setsid`, owns one PTY,
process group, VT state, bounded journal, and Pane ResourceScope, and never
transfers those resources to qwed.

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
session.layout
pane.focus
pane.reorder
pane.floating_update
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

Stage 8 slice 1 adds optional Pane targeting to `ATTACH_OPEN`. qwed validates
that the Pane belongs to the requested Session and proxies only to that Pane
worker. A compositor can therefore hold one exclusive interactive channel per
Pane. `SESSION_LAYOUT` returns viewport-relative rectangles from the shared
`qwe-core` tiling function. Pane kind, order, relative weight, z-index, and
Session focus are durable in schema v12; floating rectangles are durable in
schema v13 and viewport layouts are derived.
See [ADR 0022](adr/0022-session-layout-compositor.md) and
[ADR 0023](adr/0023-durable-pane-layout.md). Relative insertion and reorder
are authorized qwed mutations under
[ADR 0024](adr/0024-pane-composition-controls.md).

Copy mode and the quick menu remain client projections under
[ADR 0025](adr/0025-copy-and-quick-menu.md). Copy text comes from the focused
Pane's current client VT screen, never from native terminal selection or the
diagnostic journal. Snapshot replacement, resize, process exit, and Attach close
cancel active selections.

Fixed floating Panes use the same worker/Attach/resource paths under
[ADR 0026](adr/0026-fixed-floating-panes.md). Tiled geometry excludes them;
the compositor derives a centered popup and renders floating Panes afterward in
persisted z-order. [ADR 0027](adr/0027-floating-bounds-and-keyboard-move.md)
persists optional rectangles and provides `Ctrl-A d` keyboard move/resize.
[ADR 0028](adr/0028-mouse-drag-and-hit-testing.md) enables SGR button-event
tracking, performs topmost-z hit-testing, and persists floating title drags on
release. Content mouse events are forwarded only when the child VT mouse mode is
enabled. [ADR 0029](adr/0029-mouse-resize-handle.md) adds bottom-right corner
resize with the same draft and single-persist boundary.
[ADR 0030](adr/0030-floating-z-order.md) raises a floating Pane on left press
and reorders client views/targets by persisted z-index.
[ADR 0031](adr/0031-scrollback-and-mouse-copy.md) exposes bounded client VT
scrollback in copy mode and supports focused-Pane mouse selection.

Stage 3 moves attachment authority to qwed. qwed validates `ATTACH_OPEN`,
persists the active owner and SHA-256 reclaim-token hash, and only then sends
`ATTACH_ACCEPTED`. It intercepts `DETACH` to create a positive-TTL hold or an
immediate release. Reattach consumes the current Lease generation and writes a
release tombstone. Persisted Active authority becomes Reclaim Grace at qwed
startup; only the matching token can resume the Session during that window.

## 6. State Ownership

| State | Owner | Durable Storage |
|---|---|---|
| Live PTY and writer | Pane worker | No |
| VT screen and cursor | Pane worker | No |
| Recent terminal bytes | Pane worker | No |
| Task process group | Pane worker | Identity mirrored to SQLite |
| Session/Pane/TaskInstance metadata | `qwed` | SQLite |
| Pane worker identity and lifecycle | `qwed` inventory | SQLite mirror keyed by Pane |
| ResourceScope lifecycle and requested policy | `qwed` / backend | SQLite plus backend readback |
| Principal and revocation epoch | `qwed` | SQLite tombstone |
| Lease owner/deadline | `qwed` | SQLite |
| Attachment authority | `qwed` | SQLite token hash, owner, generation, deadline; keyed by Pane after v11 |
| Task history | `qwed` | SQLite |
| Pane layout metadata, focus, and floating bounds | `qwed` | SQLite schema v13 |
| Worker control, health, and inventory | `qwed` supervisor | Persistent channels keyed by Pane, rebuilt by reconciliation |
| Configuration | Workspace files | Files |

## 7. Recovery Semantics

| Failure | Expected Behavior |
|---|---|
| CLI disconnect | Session and task continue; attach later |
| `qwed` SIGKILL | All session workers continue |
| `qwed` restart | Rediscover workers and reconcile SQLite inventory |
| Corrupt SQLite | Fail explicitly; do not start with silent data loss |
| Pane worker SIGKILL | That Pane is lost; V0's one-Pane Session returns `terminal_lost`, while other Panes remain healthy after the multi-Pane migration |
| OS reboot | No V0 recovery |

On restart, qwed scans its runtime directory and reconnects workers after
validating the expected socket path, SessionId, PaneId, worker PID, and start
token. Recovery never reconstructs a terminal by replaying an unbounded byte
stream.

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
- SQLite schema v11 removed the one-Pane/one-Task uniqueness assumptions and
  stores Pane-worker, ResourceScope, and Principal records alongside
  Pane-keyed Tasks and attachment authority. `CONTROL_STATUS` exposes all
  inventory records.
- Schema v12 adds durable Pane kind, layout order, relative weight, z-index,
  and Session focus. Active layout membership includes only Pane workers in
  `active` or `exited` state; closed/lost history remains queryable but does
  not drive composition. Schema v13 adds optional floating rectangles and is
  the current schema.
- Lease release writes a generation tombstone; expiration does not kill Tasks.

## 10. Resource Model

- ResourceScope targets Session, Pane, or Task. The first Stage 7 Linux
  implementation targets the Pane; a Session is the logical parent.
- Each Pane has a tree-scoped policy for memory high/max/swap,
  processes-plus-threads, CPU bandwidth, and cumulative CPU time.
- Linux creates one transient systemd user scope per Pane, named from the
  stable Pane identity. V0 still instantiates one Pane per Session; the scope
  boundary is already Pane-level. `memory.max`, `memory.swap.max`, `pids.max`,
  and `cpu.max` are hard limits; cumulative CPU time is monitored.
  `memory.oom.group=1` treats the selected ResourceScope as one OOM unit.
- Inside that scope, the worker uses delegated `control` and `workload`
  children. Resource limits apply to `workload`, which contains the PTY task
  and agent commands; worker threads remain in `control` and do not consume
  the user's task allowance.
- The topology acknowledgement blocks command exec until the worker owns the
  cgroup, has installed its event watch, and has applied the initial policy.
  The production Linux path starts the worker itself inside the scope; the
  legacy nested `qwe-session-exec` topology remains available to worker-only
  probes.
- macOS traverses the task tree with `proc_listchildpids` and samples
  `proc_pid_rusage` every 100 ms. Memory, task-count, and CPU limits are
  `monitored`; a breach terminates the process group.
- Status reports requested/effective policy, current and peak memory/task
  usage, remaining limits, cumulative CPU time, enforcement tier, health,
  controller counters, PSI, and bounded enforcement events.
- Linux watches cgroup event files through inotify so OOM and fork-limit events
  are retained before systemd removes a transient scope. Event history is
  in-memory and capped per Pane worker.
- Dynamic updates replace selected fields on a running Active/Held Session.
  Raising limits is immediate. Lowering a Linux hard memory limit can reclaim
  or OOM immediately; lowering `pids.max` does not kill existing tasks but
  blocks new forks until usage falls below the limit.
- V0 does not claim hard tree limits when systemd user scopes are unavailable.
- Complete Pane-scope teardown stops the systemd scope so detached interactive
  job groups are removed in addition to the task process group. A scope is
  tombstoned only after the backend confirms that no owned process or
  controller state remains.

GPU-to-task attribution remains outside the Pane resource contract and is
reported only when a reliable platform identifier exists.

## 11. Security Baseline

- One local OS user for V0.
- V0 has one interactive control client per Session. The accepted target has
  one interactive writer per Pane and a separate Session Lease for lifecycle
  control.
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
- ChannelProfile separates projection and authorization from state ownership.
  `human_observer` is a qwed read-only projection; it cannot acquire input,
  resize, or lifecycle authority. AuthContext remains opaque to IPC clients.
- Menu keys never enter PTY input. Clipboard provider failures are explicit and
  are never reported as successful copies.
- Principal records use monotonic epochs and tombstones. They do not elevate
  the V0 same-UID owner string into an authenticated identity.
- Capability tokens are random, expiring values stored as SHA-256 hashes.
  Capability scope is global, Session, Pane, or command, and each record pins
  the complete Principal ancestor epoch snapshot. Every agent request reloads
  that chain and fails on stale epochs or any revoked ancestor.
- The local-root bootstrap management capability is written to a mode-0600
  runtime file; agent tokens are read from explicit mode-0600 capability files.
  Raw capability values are never stored in SQLite or passed to child argv or
  environment.
- Revocation is explicit. Administrative revocation invalidates authorization
  while preserving running work; freeze stops affected command groups and
  expires into termination; terminate kills them. Missing severity fails safe
  to freeze.
- Per-command credentials travel from the private provider through a one-shot
  mode-0600 broker using Unix `SCM_RIGHTS`, then map to child FD3. The worker
  keeps the same secret only in `Zeroizing` memory for streaming redaction
  before event retention. Secret values never enter JSON, argv, environment
  values, SQLite, event data, or logs.
- Command observer subscriptions are qwed read-only projections over committed
  worker events. They carry a bounded queue, recheck the live capability on
  every poll, and have no command mutation operation.

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

The Stage 0 design gate, automated Stage 1-3 acceptance, and Stage 6.5 shared
prerequisites are closed. These decisions remain open and must be recorded in
ADRs before implementation:

1. Final VT engine after broader compatibility benchmarks.
2. Exact idle-timeout defaults for non-terminal Sessions.

These decisions must be recorded as accepted ADRs before their implementation.
