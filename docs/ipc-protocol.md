# QWE IPC Protocol

Status: Multi-Pane inventory and control are implemented. Stage 8 slices 1-10
add Pane-targeted Attach, composition, durable layout, and copy controls.
Date: 2026-11-11

This document describes the currently implemented request/response protocol
and the accepted Stage 6.5 target where explicitly labeled. The accepted V0
Attach frames and state contracts are canonical in
[`protocol/schema/qwe-v1.schema.json`](../protocol/schema/qwe-v1.schema.json).
ADR [0011](adr/0011-consolidate-control-plane.md) defines the supervisor and
persistent worker-control model. ADRs
[0012](adr/0012-terminal-attach-data-plane.md) and
[0013](adr/0013-core-object-model.md) define the Attach and object contracts.

## 1. Authority Boundaries

- `qwed` is the sole authority for SQLite Sessions, Tasks, and Leases.
- `qwed` owns live worker inventory, control channels, and health state. V0
  inventory is keyed by Session; the accepted multi-Pane target keys by Pane.
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
`SESSION_ATTACH`, `SESSION_INPUT`, `TASK_STOP`, `TASK_KILL`, `SESSION_CLOSE`,
and `SESSION_KILL`. The
request/response channel is persistent and supports concurrent in-flight
requests keyed by `request_id`. `PING` is a lightweight probe and does not
capture the terminal journal.

`SESSION_INPUT`, `TASK_STOP`, `TASK_KILL`, `SESSION_CLOSE`, and `SESSION_KILL`
require the current Active owner. A Held Session requires the owner and Lease
generation. Unattached and Reclaim Grace Sessions return `not_attached`. The
CLI can set the logical owner with global `--owner` and provides `--generation`
on close, task-stop, and forced teardown.
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

`read_only` opens a qwed-owned observer channel. The observer receives a
snapshot and subsequent live output mirrored from the Pane's exclusive
interactive channel, but cannot send input, resize, detach, or lifecycle
control. Immediate `DETACH` closes the interactive channel and releases
authority. A positive-TTL `DETACH` closes the channel and creates a
Lease-backed Hold. Reattach requires the current owner and Lease generation
and writes a release tombstone before returning Active.

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
It also returns `capabilities`, credential backend/names, and the control frame
and input limits.

## 6. Stage 4 Capabilities

`SESSION_CREATE` accepts optional `credential` and `resource_policy` fields.
The credential name selects a private credential-directory entry or a macOS
Keychain item named `<prefix>.<name>`. Secret bytes never appear in the
request, response, environment, argv, logs, or SQLite. The final PTY child sees
the secret through inherited descriptor 3 and `QWE_CREDENTIAL_FD=3`.

`resource_policy` accepts:

- `memory_high_bytes`
- `memory_max_bytes`
- `memory_swap_max_bytes`
- `tasks_max`
- `cpu_quota_percent`
- `cpu_seconds`

The legacy JSON names `memory_bytes` and `process_limit` are accepted as aliases
for `memory_max_bytes` and `tasks_max`.

Linux starts each Session in a transient systemd user scope and reports hard
cgroup v2 memory, swap, task-count, and CPU-bandwidth controls. Cumulative CPU
seconds are monitored. macOS reports libproc tree observation and monitored
termination with bounded polling overshoot. Hard requests without the required
platform tier return `unsupported_capability`.

`SESSION_RESOURCE_UPDATE` patches the policy of a running Session. Fields
omitted from `resource_policy` are unchanged; `null` clears a field. The request
requires Active or Held authority, is idempotent, applies runtime policy before
durably updating the Pane, and rolls the runtime back if persistence fails. Its
response contains the durable policy and terminal status, including current and
peak metrics. The CLI is `qwe session limits`.

When lowering `memory_max_bytes` without explicitly supplying
`memory_high_bytes`, the existing high threshold is clamped to the new maximum.

Forced teardown operations are:

- `SESSION_KILL`: authorize like `SESSION_CLOSE`, then `SIGKILL` the Task
  process group and persist `closed` history.
- `TASK_KILL_BATCH`: accept 1-100 unique TaskIds, authorize each Session, and
  return independent success/failure items.
- `SERVER_KILL`: require `confirm=true`, force-kill every non-closed Session,
  return the summary, and exit qwed.

The CLI names are `kill-session`, `task kill`, and `kill-server`.

## 7. Lease State Machine

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

## 8. Idempotency

Mutating operations require a non-empty `idempotency_key` of at most 256 bytes.
The key is scoped by the logical operation and canonical request payload.

- Read-only operations never create idempotency records, even if a key is
  supplied.
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

## 9. Backpressure

Each Session has one bounded 16 MiB journal. One-shot snapshot returns at most
1 MiB of diagnostic journal tail. Each live attachment has an independent
1 MiB output queue and closes only that channel on overflow; no silent output
loss is permitted.

## 10. Multi-Pane Contract

[ADR 0021](adr/0021-shared-prerequisites.md) accepts Pane-targeted channels and
Pane-keyed worker inventory. Schema v13 and qwed's Pane-keyed control operations
are implemented. The public `qwe-v1` binary Attach resolves the Session's
default Pane when `pane_id` is omitted. Stage 8 slices 1-10 add direct routing
and Pane composition controls. qwed-owned interactive and observer channels
can coexist on one Pane while only the interactive channel holds write
authority.

### Target Selection

- `ATTACH_OPEN` carries `session_id` and an optional `pane_id`. qwed verifies
  the Pane belongs to the Session before selecting its worker and attachment
  authority.
- `PANE_CREATE` carries `session_id`; other Pane control operations carry
  `pane_id`. qwed resolves the owning Session from durable state.
- `PANE_CREATE.after_pane_id` inserts immediately after a live source Pane;
  omission appends. `PANE_REORDER` replaces the complete ordered live Pane
  list transactionally.
- `PANE_CREATE.pane_kind=floating` creates a full worker-backed overlay with an
  increasing z-index. Floating Panes reject `after_pane_id` and do not
  participate in `PANE_REORDER`.
- `PANE_FLOATING_UPDATE` saves a complete floating rectangle under
  LayoutMutation authority and rejects tiled Panes.
- `PANE_FOCUS` persists the Session focus preference. It requires Active owner
  authority on the target Pane, Held owner plus generation, or a current
  Session Lease when the target is unattached.
- qwed resolves the Pane worker from inventory and rejects a missing, lost, or
  unreachable Pane before opening a worker channel.
- Clients never supply a worker socket path or select a worker by Session
  order.
- Pane-scoped interactive and observer attachment state is durable. Session
  Leases remain the fence for Session-wide lifecycle operations.

### Layout Query

`SESSION_LAYOUT` accepts `session_id`, `rows`, and `columns`. It is read-only
and returns Pane identity, worker state, focus, layout order, relative weight,
z-index, and non-overlapping viewport-relative rectangles. The current
implementation uses deterministic near-square tiling for normal Panes and is
bounded to 64 Panes. Floating Panes return a centered popup rectangle and are
ordered after tiled Panes by z-index.

Schema v12 persists Pane order, weight, z-index, and Session focus; v13 adds
optional floating rectangles. An explicit
`focused_pane_id` is a request hint; otherwise persisted focus is used, then
the first ordered Pane. Default Attach resolves the same persisted focus before
layout order.

`qwe session layout` exposes the response. `qwe session compose` uses it to
open one Pane-targeted Attach channel per Pane and routes input only to the
focused Pane. Existing `qwe session attach` continues to target the default
Pane. See [ADR 0022](adr/0022-session-layout-compositor.md).

Closed and lost Pane-worker history is excluded from `SESSION_LAYOUT` and
`PANE_REORDER` membership. The compositor uses `Ctrl-A c` to split after focus,
`Ctrl-A x` to close focus, and `Ctrl-A h/l` to move focus left/right. The last
visible Pane cannot be closed by `Ctrl-A x`; close its Session instead. See
[ADR 0024](adr/0024-pane-composition-controls.md).

`Ctrl-A f` creates a centered fixed floating shell. Floating Panes retain
independent workers, PTYs, and ResourceScopes and can be focused or closed
normally. Title dragging, corner resizing, and topmost-z hit-testing are
implemented. See
[ADR 0026](adr/0026-fixed-floating-panes.md).

`Ctrl-A d` enters keyboard move mode for the focused floating Pane. `h/j/k/l`
moves, `H/L` changes width, `K/J` changes height, `y` saves, and `q`/Escape
cancels. Saved rectangles survive restart and are clamped by the client to the
usable viewport. See
[ADR 0027](adr/0027-floating-bounds-and-keyboard-move.md).

The compositor enables SGR button-event tracking (`1002`/`1006`) while attached.
Left-button drag on a floating title row persists bounds on release. Pane
hit-testing walks the rendered stack in reverse z-order. Content mouse events
are forwarded to the target Pane only when its VT screen enabled a mouse
protocol. See [ADR 0028](adr/0028-mouse-drag-and-hit-testing.md).

A visible bottom-right marker is the floating resize handle. Dragging it
changes only the client draft width/height and persists one rectangle on
release. See [ADR 0029](adr/0029-mouse-resize-handle.md).

Left press on any floating Pane calls `PANE_FLOATING_RAISE`, persists a new
z-index, and reorders the client view/Attach stack before drag or content
routing. See [ADR 0030](adr/0030-floating-z-order.md).

Copy mode Ctrl-U/Ctrl-D navigates bounded client VT scrollback. Mouse
press/motion/release inside focused-Pane content creates a Pane-clamped
selection and copies on release. Attach replacement or compositor restart
discards selection and client scrollback. See
[ADR 0031](adr/0031-scrollback-and-mouse-copy.md).

`Ctrl-A [` enters focused-Pane visible-screen copy mode and `Ctrl-A m` opens
the client quick menu. Copy and menu state are not protocol objects. Selection
is clamped to the focused Pane's current client VT screen; snapshot
replacement, resize, process exit, and Attach close cancel it. Clipboard
success names the provider; failure is explicit. Menu bytes are consumed before
PTY input. See [ADR 0025](adr/0025-copy-and-quick-menu.md).

### Channel Profiles

The accepted profiles are `human_interactive`, `human_observer`, `agent_exec`,
`agent_stream`, and `agent_observer`. Profile, projection, capability
allowlist, replay/backpressure policy, and opaque AuthContext are channel
inputs. The detailed profile table and authorization boundary are in
[ADR 0021](adr/0021-shared-prerequisites.md).

`human_observer` is a qwed-owned read-only projection over an exclusive
internal worker Attach channel. It can subscribe to snapshot and output but
cannot send input, resize, detach, or lifecycle control. qwed rejects those
frames before they reach the worker. The worker does not need a native
`read_only` Attach mode; qwed maps the public observer profile onto its
internal exclusive worker channel and fans committed frames out to observer
subscribers.

### Inventory, Recovery, and Resource Reporting

- Worker discovery, adoption, health, loss, and cleanup are addressed by Pane.
- A Pane worker loss does not mark unrelated Panes in the same Session lost.
- Resource protocol fields expose target, requested/effective policy, bounded
  usage, pressure, enforcement events, lifecycle, and cleanup state.
- cgroup paths, systemd unit names, `/proc` details, and backend credentials
  never become public fields.
- Detached interactive job groups belong to the Pane scope and must be cleaned
  before the scope is tombstoned.

### Compatibility

Optional `pane_id` and `SESSION_LAYOUT` are additive: clients that omit the
field continue to use the default Pane. A protocol change that removes the
default or requires every client to understand multiple workers must introduce
a new negotiated protocol version, update [Storage Migration](storage-migration.md),
and pass the applicable shared-prerequisite cases before replacing the
compatible path.

## 11. Security Model

Owner-only sockets prevent other UIDs from connecting at the OS layer.
Application peer-UID checks remain defense in depth for misconfigured paths,
future group-accessible sockets, and compromised file permissions.
Client frames are limited to 8 MiB before payload allocation, and control
input is limited to 1 MiB. Malformed and oversized frames close only the
offending client connection.
