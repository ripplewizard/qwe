# QWE Design Baseline v0.4

Status: Superseded by Baseline v0.5
Version: 0.4
Date: 2026-11-03

## 1. Purpose

This baseline supersedes v0.3 after validating Host crash recovery, persistent
control state, startup reconciliation, and concurrent Attach.

## 2. Confirmed Decisions

| ID | Decision |
|---|---|
| D016-D027 | Retain the Rust, protocol, identity, storage, and scope decisions from v0.3. |
| D028 | Each Session's PTY, VT state, and journal are owned by an isolated session worker. |
| D029 | `qwe-host` is restartable and rediscovers surviving workers through its runtime directory. |
| D030 | A worker crash loses only that Session and returns `terminal_lost`. |
| D031 | SQLite WAL schema v2 stores Sessions, Tasks, process identity, exit status, and leases. |
| D032 | `qwed` reconciles SQLite against Host inventory before accepting clients. |
| D033 | Session lease changes require owner and monotonically increasing generation. |
| D034 | Attach is concurrent; the release gate is 8 clients under continuous PTY output. |
| D035 | Attach P95 must remain at or below 3,000 ms and P99 at or below 5,000 ms. |

## 3. Process Architecture

```text
qwe CLI
  |
  v
qwed (control plane) --- SQLite WAL
  |
  v
qwe-host (recoverable manager)
  |
  +-- session worker: PTY, VT, journal, process group
  +-- session worker: PTY, VT, journal, process group
```

Workers call `setsid` and expose owner-only Unix sockets. Host and worker
processes remain independent of `qwed`. A killed Host is replaced by a new Host
that rescans the private runtime directory and reconnects to surviving workers.

## 4. Recovery Matrix

| Failure | Result |
|---|---|
| CLI disconnect | Session continues and may reattach |
| `qwed` SIGKILL | Host and workers continue; restart reconciles SQLite |
| `qwe-host` SIGKILL | Workers continue; restarted Host rediscovers them |
| One worker SIGKILL | That Session is lost; other Sessions continue |
| Corrupt SQLite | Explicit startup failure |
| OS reboot | No V0 recovery |

## 5. Control State

SQLite stores:

- Session IDs, owner, state, and process identity.
- Task UUIDv7, Session association, PID/PGID/start token, and exit status.
- Lease owner, generation, deadline, and update time.

Startup reconciliation marks persisted Sessions and Tasks `lost` when they are
absent from Host inventory. Natural exits update task exit code and signal when
observed by status or reconciliation.

## 6. Terminal and Concurrency

Each worker retains at most 16 MiB of PTY output and returns at most a 1 MiB
journal tail on Attach. `SESSION_STATUS`, input, and detach remain lightweight.

`qwed`, Host, and workers accept connections concurrently. Shared state is not
held while writing a bounded snapshot response. The automated gate runs 64
Attach requests with eight concurrent clients during continuous output.

## 7. Verification

- `just check-qwe` runs formatting, Clippy, unit tests, migrations, and identity
  checks.
- `just t12-host-smoke` verifies daemon failure and 100 MiB bounded output.
- `just resilience-smoke` verifies Host failure, worker persistence, Lease/Task
  recovery, startup reconciliation, concurrent Attach, and percentile latency.

## 8. Remaining Decisions

- Whether read-only observers are supported.
- Final VT engine after broader compatibility benchmarks.
- Exact idle-timeout behavior for non-terminal Sessions.

## 9. Current Implementation Gaps

- `SESSION_RESIZE` is not yet exposed by the production host protocol.
- Attach does not yet expose complete cursor and terminal-mode state.
- Idempotency keys are transported but not durably deduplicated.
- systemd and launchd lifecycle integration is pending.
- Release-mode soak and adversarial security suites are pending.

These gaps are active development work. Manual POC sign-off remains pending but
does not block implementation.

## 10. Deferred Scope

Multi-node execution, mesh, sandboxing, credential brokering, agent runtime,
and GPU isolation remain deferred.
