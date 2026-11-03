# QWE Design Baseline v0.3

Status: Superseded by Baseline v0.4
Version: 0.3
Date: 2026-11-03

## 1. Purpose

This baseline converts the completed POC into production architecture decisions.
It supersedes v0.2 for implementation work while retaining the v0.2 product
boundaries.

## 2. Confirmed Decisions

| ID | Decision |
|---|---|
| D016 | Rust is the primary implementation language. |
| D017 | PTY and process ownership lives in a durable `qwe-host`, not `qwed`. |
| D018 | `qwed` is a restartable control plane over versioned local IPC. |
| D019 | V0 uses `portable-pty` and `vt100` behind replaceable interfaces. |
| D020 | Attach uses a bounded byte journal plus compact VT state. |
| D021 | IPC uses length-prefixed, versioned JSON frames with peer credentials. |
| D022 | Task identity combines an opaque TaskId with OS process birth identity. |
| D023 | SQLite WAL stores metadata and history, not high-frequency terminal bytes. |
| D024 | V0 supports one local OS user, one daemon, and one host per user. |
| D025 | V0 targets Linux x86_64 and macOS ARM64. |
| D026 | Host crash loses live sessions; OS reboot recovery is out of scope. |
| D027 | Mesh, Sandbox, Credential Broker, and Agent Runtime remain deferred. |

## 3. Process Architecture

```text
qwe CLI
  |
  | versioned IPC
  v
qwed (control plane) --- SQLite WAL
  |
  | host control protocol
  v
qwe-host (durable runtime owner)
  |
  +-- Session/PTY/VT state
  +-- Task process groups
  +-- bounded output journals
```

`qwed` may be killed and restarted without losing terminal sessions or task
process groups. `qwe-host` owns all live execution resources. CLI connections
never own a PTY or process group.

## 4. Recovery Matrix

| Failure | Survives | Recovered |
|---|---|---|
| CLI disconnect | Yes | Reattach |
| `qwed` SIGKILL | Yes | New `qwed` rediscovers host |
| Corrupt SQLite | No | Explicit startup failure |
| `qwe-host` SIGKILL | No | Shell reports terminal loss |
| OS reboot | No | No V0 recovery |

Daemon restart must preserve session IDs, process identity, accepted input,
bounded recent output, VT snapshot, ownership deadline, and task history.

## 5. Terminal State

The terminal path separates:

1. PTY transport: `portable-pty`.
2. VT state: `vt100` behind a QWE-owned interface.
3. Session and attach: owned by `qwe-host`.
4. Rendering: CLI/TUI responsibility, initially plain terminal streaming.

V0 retains at most 16 MiB of recent PTY output per session. Attach requests
receive a VT snapshot plus bounded recent bytes, never an unbounded replay.
Truncation is explicit using `truncated_from`.

## 6. Process and Task Identity

- Every task receives an opaque UUIDv7 `TaskId`.
- Local identity records PID, process-group ID, and a platform start token.
- Linux uses boot ID, `/proc` start time, and PID.
- macOS uses `proc_pidinfo` start seconds/microseconds, not second-only `ps`.
- Task history records exit code, signal, started/ended timestamps, and restart
  policy outcome.

## 7. IPC and Security

- Unix Domain Socket only.
- Length-prefixed JSON frames.
- Protocol version is mandatory and mismatches return `incompatible_version`.
- Request IDs are required; mutating requests require idempotency keys.
- Socket permissions are owner-only.
- Peer UID must match the server UID.
- Request and frame sizes are bounded.

## 8. Persistence

SQLite WAL stores sessions, tasks, process identity, leases, topology, and
history. Terminal bytes and screen snapshots are not written on every frame.
Offline snapshots may be checkpointed periodically or on detach.

Schema migration is monotonic and versioned. A failed migration leaves the old
database intact.

## 9. Workspace Boundaries

```text
crates/qwe-core       IDs, objects, errors, invariants
crates/qwe-protocol   Versioned IPC and host framing
crates/qwe-storage    SQLite migrations and repositories
crates/qwe-runtime    Bounded journal and runtime interfaces
apps/qwed             Control daemon
apps/qwe-host         Durable PTY/process owner
apps/qwe              CLI client
```

No crate may bypass `qwe-protocol` to access another process. No module may
depend directly on private storage rows; repositories map rows to domain types.

Detailed dependency and protocol guidance is in
[`qwe-technical-architecture.md`](qwe-technical-architecture.md).

## 10. V0 Scope

P0:

- Session/Window/Pane and terminal attach/detach.
- Durable host ownership across daemon restart.
- Bounded output and compact VT attach.
- Process groups, signals, timeout, identity, and history.
- SQLite metadata, leases, migrations, and explicit errors.
- Versioned same-user Unix IPC.
- Basic CPU/memory monitoring.

P1:

- Just integration and configuration commands.
- TUI.
- macOS/Linux capability parity.

Deferred:

- Multi-node execution, QWE Mesh, distributed scheduling.
- Security sandbox, credential broker, agent runtime.
- GPU isolation and full environment reproduction.

## 11. Acceptance Gates

Production implementation cannot claim completion until:

1. `qwe-host` survives `qwed` SIGKILL and restores session state.
2. 100 MiB output cannot exceed the configured memory budget.
3. Attach returns a bounded snapshot and accepts input.
4. PID reuse cannot misidentify an existing process.
5. Mixed protocol versions fail explicitly.
6. Unauthorized local clients cannot connect.
7. Clean Linux and macOS builds pass the same integration suite.
