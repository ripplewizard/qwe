# ADR 0009: Control-Plane Authority and Runtime Reconciliation

Status: Accepted
Date: 2026-11-04

## Decision

`qwed` is the sole authority for SQLite Session, Task, and Lease state.
`qwe-host` is the authority for live worker and PTY/process health.

`qwed` reconciles Host inventory at startup and every 250 ms while running.
Host inventory validates each worker connection. A missing or unresponsive
worker is removed and the corresponding Session/Task becomes `lost`.

Host restart accepts a worker only when its socket path, Session ID, worker PID,
and PID start token all match. Lease release writes a generation tombstone
instead of deleting the row; expiration changes control authority but does not
kill Tasks.

## Consequences

- Runtime worker death becomes visible without waiting for a daemon restart.
- PID reuse cannot attach an old runtime socket to a new process.
- Lease generations stay monotonic across release and takeover.
