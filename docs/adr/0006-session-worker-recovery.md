# ADR 0006: Session Worker Recovery

Status: Accepted
Date: 2026-11-03

## Context

If `qwe-host` directly owns the PTY master and is killed, closing that file
descriptor terminates the terminal path even when the shell survives briefly.
Host as the sole durable owner therefore cannot recover from its own crash.

## Decision

Each Session is owned by an isolated session worker process. The worker owns
the PTY, File descriptor, VT state, process group, and bounded journal. Workers
call `setsid` and expose an owner-only Unix socket under a per-host runtime
directory.

`qwe-host` is a recoverable manager. It starts workers and forwards requests,
then rescans the runtime directory after restart and reconnects to surviving
workers. A dead worker socket is removed during recovery.

## Consequences

- `qwe-host` crash no longer loses live Sessions.
- Worker crash loses that Session and is reported as `terminal_lost`.
- Runtime directories must be private and cleaned after Session closure.
- Host shutdown does not implicitly destroy worker-owned Sessions.
