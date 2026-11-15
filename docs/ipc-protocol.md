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

### Security Capability Control

Capability-authorized requests carry:

```json
{"auth_context":{"capability":"<opaque-token>"}}
```

The token is sent only inside the authenticated owner-only control socket
envelope. It must not appear in operation payloads, argv, environment, SQLite,
agent events, or logs. `RequestEnvelope.auth_context` is optional for legacy
operations and required by security and agent operations.

| Operation | Required payload |
|---|---|
| `SECURITY_BOOTSTRAP` | none; requires an active local-root management capability |
| `SECURITY_PRINCIPAL_CREATE` | `parent_id`, optional `high_trust` |
| `SECURITY_CAPABILITY_ISSUE` | `principal_id`, `operation`, `scope`; optional `ttl_ms`, `sandbox_profile` |
| `SECURITY_PRINCIPAL_REVOKE` | `principal_id`; optional `severity`, `grace_ms`, `hold` |

Capability operations and scopes:

| Operation | Accepted scope |
|---|---|
| `manage_principals` | `{"kind":"global"}` |
| `agent_exec` | `{"kind":"pane","pane_id":"..."}` |
| `agent_command_read` | `{"kind":"command","pane_id":"...","command_id":"..."}` |
| `agent_command_terminate` | `{"kind":"command","pane_id":"...","command_id":"..."}` |

Capability issue is non-idempotent so the raw token is never copied into the
durable idempotency response table. Tokens default to 24 hours and are capped at
30 days; the local-root bootstrap management token uses the 30-day maximum.
SQLite stores only token hashes plus the Principal's complete ancestor epoch
snapshot.

`sandbox_profile` is valid only for an `agent_exec` capability. Its presence
makes the capability sandboxed; omission preserves the explicit unsandboxed
path. The v1 profile shape is:

```json
{
  "version": 1,
  "workspace": "/absolute/existing/directory",
  "workspace_mode": "read_only",
  "protected_paths": ["/absolute/existing/path/inside/workspace"],
  "network": "isolated_loopback",
  "home": "hidden",
  "private_tmp": true,
  "process_namespace": "isolated",
  "seccomp": {"mode": "none"}
}
```

`workspace_mode` is `read_only` or `read_write`; `network` is
`isolated_loopback` or `inherit`; `home` is `hidden` or `read_only`; and
`process_namespace` is `isolated` or `shared`. Unknown fields, non-v1 profiles,
named seccomp values other than the exact profile name `strict`, a missing
workspace, or a missing protected path fail closed.
Sandboxed PTY execution preserves terminal semantics and applies an internal
`TIOCSTI`-denying seccomp filter; the profile's `seccomp:none` value does not
disable that adapter hardening.

The optional profile value `"seccomp":{"mode":"named","profile":"strict"}`
adds a deny-focused cBPF layer. It rejects `ptrace`, `bpf`, `keyctl`,
`add_key`, `request_key`, and `userfaultfd` with `EPERM`; it rejects
`io_uring_setup` with `ENOSYS` so runtime feature detection can fall back.
`strict` is a stable profile name, but it is not a syscall allowlist or a
complete syscall sandbox. Unknown names are rejected rather than downgraded.

A sandboxed capability also returns `sandbox_audit` with `profile_hash`,
`seccomp_profile_hash`, `backend_path`, `backend_version`, and
`probe_time_ms`. The seccomp hash is the SHA-256 of the exact cBPF bytes passed
to Bubblewrap. These values are stored with the capability and included in
live and recovered sandbox status.

`SECURITY_PRINCIPAL_REVOKE.severity` is `administrative`, `freeze`, or
`terminate`; omission defaults to `freeze`. Freeze grace defaults to 60000 ms
and is capped at 300000 ms. Expiry escalates to termination. Administrative
revocation and `hold: true` require a high-trust Principal. The single local
root bootstrap token is written by qwed to
`<socket>.runtime/security.bootstrap` with mode `0600`.

Authorization failures use:

`capability_missing`, `capability_invalid`, `capability_expired`,
`capability_revoked`, `capability_epoch_stale`, `capability_scope_denied`,
`principal_revoked`, and `insufficient_trust`.

Sandbox failures use `sandbox_backend_unavailable`,
`sandbox_profile_unsupported`, and `unsupported_platform`. `AGENT_EXEC` rejects
a client-supplied `sandbox_profile` with
`sandbox_profile_not_client_selectable`; a request cannot enable, replace, or
remove the capability's enforced profile.

Error bodies may include an optional backward-compatible `details` object:

```json
{
  "code": "sandbox_backend_unavailable",
  "message": "refusing sandboxed agent_exec capability: ...",
  "details": {
    "reason": "mount_denied",
    "diagnostics": {
      "environment": "container",
      "failed_stage": "mount",
      "target": "/proc",
      "backend_version": "bubblewrap 0.9.0",
      "probe_time_ms": 1794700000000,
      "stderr": "bwrap: Can't mount proc on /newroot/proc: Operation not permitted"
    }
  }
}
```

Stable sandbox reason values are `bwrap_missing`,
`bwrap_version_incompatible`, `userns_disabled`, `userns_create_denied`,
`uid_map_denied`, `mount_denied`, `probe_timeout`, `unsupported_platform`, and
`unknown`. `details.diagnostics` is diagnostic context, not a stable protocol
surface. Diagnostics are bounded and sanitized, and clients must treat unknown
reason values as `unknown`.

High-trust Principals receive the full diagnostics object. Other authenticated
Principals receive only `environment`, `failed_stage`, and `target`; stderr,
backend paths/version, probe time, and sysctl snapshots remain restricted.

`AGENT_EXEC` startup failures expose the same `{reason, diagnostics}` shape as
`error_details` in command status/wait responses.

### Agent Command Control

Stages 9.1-9.5 add the following capability-protected control operations.
Command IDs are UUIDv7 text values and are stable across qwed restart while the
owning worker survives.

| Operation | Required payload |
|---|---|
| `AGENT_EXEC` | `pane_id`, `program`, `args[]`; optional `cwd`, absolute `deadline_ms`, `owner`, `credential` |
| `AGENT_COMMAND_STATUS` | `command_id` |
| `AGENT_COMMAND_EVENTS` | `command_id`; optional `cursor`, `limit` |
| `AGENT_COMMAND_WAIT_IDLE` | `command_id`; optional `timeout_ms`, maximum 30000 |
| `AGENT_COMMAND_TERMINATE` | `command_id` |
| `AGENT_COMMAND_SIGNAL` | `command_id`, `signal` |
| `AGENT_COMMAND_OBSERVE` | `command_id`; optional `cursor`, `timeout_ms`, maximum 30000 |

`AGENT_EXEC`, `AGENT_COMMAND_TERMINATE`, and `AGENT_COMMAND_SIGNAL` are mutating
operations and require an idempotency key. `program` is one executable name or
path; QWE does not interpret `args` as shell syntax. `cwd` must name an existing
directory for spawn to succeed. The caller must start a shell explicitly when
shell syntax is wanted.
`sandbox_profile` is not an accepted `AGENT_EXEC` request field; qwed injects
the internal profile from the capability only after authorization.

`AGENT_COMMAND_SIGNAL.signal` accepts only `SIGINT`, `SIGTERM`, `SIGHUP`, and
`SIGQUIT`. The signal targets the current PTY foreground process group when
available, otherwise the command process group. `SIGKILL` and arbitrary signal
numbers are rejected; forced termination remains exclusively
`AGENT_COMMAND_TERMINATE` plus Pane cgroup teardown.

`AGENT_EXEC` requires an `agent_exec` capability for the exact Pane. The returned
command records the creating capability and Principal. That same `agent_exec`
capability may read, wait for, query events from, or terminate only commands it
created. A separate command-scoped `agent_command_read` or
`agent_command_terminate` capability may be issued to another Principal.

`pty:true` produces one merged `pty` stream and accepts optional `rows` and
`columns`; omitting `pty` produces separate stdout/stderr streams. Supplying
`rows` or `columns` without `pty:true` is invalid. `env` mutation fails
explicitly with `unsupported_capability`.

`credential` names an existing private provider credential. qwed replaces it
before worker dispatch with a one-shot `credential_socket` and
`redaction_policy=credential_exact`; these are internal worker fields. The
secret is transferred as a Unix `SCM_RIGHTS` descriptor, mapped to child FD3,
and exposed only through `QWE_CREDENTIAL_FD=3`. Missing, empty, oversized, or
unreadable credentials fail closed.

Output is redacted before event retention on each stdout/stderr or merged PTY
stream. Matching is exact and streaming, including secrets split across chunks;
replacements use `[REDACTED]` and the first match emits `redaction_applied`. Raw credential
values never enter argv, environment values, command metadata, idempotency
responses, SQLite, event data, or logs.

`AGENT_EXEC` returns the command handle immediately. The handle contains
`command_id`, Session/Pane identity, owner, worker and optional process
identities, state, output mode, timestamps, exit result, cursors, and replay
start. States are `starting`, `running`, `idle`, `done`, `failed`, and `lost`.
Sandboxed commands additionally expose `sandbox.risk` flags for
`network_inherit`, `writable_workspace`, and `credential_exposed`, plus derived
`credential_network_exfiltration` and `credential_workspace_persistence`.
The base risk flags and capability audit metadata are durable in SQLite.

`AGENT_COMMAND_WAIT_IDLE` returns `idle`, `done`, `failed`, `lost`, or `unknown`.
Timeout returns `unknown`, never `idle`. A terminal state takes precedence over a
timeout. Interactive shell PTY commands are idle when the shell's own process
group is the PTY foreground group; one-shot and command-string modes do not
claim idle.

`AGENT_COMMAND_EVENTS` returns events after the supplied cursor. Output events
carry `stream` (`stdout`, `stderr`, or `pty`), a byte `cursor`, and `data_hex`. Event
order is guaranteed within a stream. Retention is approximately 1 MiB per
command; reading an evicted cursor returns an object in `replay_gap` and never
renumbers retained events.
Sandboxed commands emit `sandbox_risk_observed` when any network, writable
workspace, or credential-exposure flag is present.

`AGENT_COMMAND_OBSERVE` is the push projection. It requires
`agent_command_read` for the exact command, or the creating `agent_exec`
capability. The server writes one or more frames with the same request ID:
`{"type":"batch",...}` followed by `{"type":"end",...}`. Each loop rechecks the
current Principal ancestry. The server keeps a bounded 32-frame queue; a slow
observer is disconnected instead of forcing unbounded buffering. The observer
has no input, resize, signal, lifecycle, or mutation authority.

The current qwed persistence stores command metadata only. Program arguments,
environment values, credentials, output bytes, and replay events are not stored
in SQLite. If the worker is confirmed absent, nonterminal commands become
`lost`; temporary worker unreachability is returned as a transport error.
Capability, revocation, credential/redaction, observer push, PTY output, and
foreground shell idle are implemented. `shell_exec`, sandbox backends,
cross-Pane orchestration, and environment mutation remain deferred and are
tracked in [Agent Execution Model](agent-execution-model.md).

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
