# QWE IPC Protocol

Status: Stages 1-3 validated; Stage 4 core capabilities implemented on macOS
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

## 10. Security Model

Owner-only sockets prevent other UIDs from connecting at the OS layer.
Application peer-UID checks remain defense in depth for misconfigured paths,
future group-accessible sockets, and compromised file permissions.
Client frames are limited to 8 MiB before payload allocation, and control
input is limited to 1 MiB. Malformed and oversized frames close only the
offending client connection.
