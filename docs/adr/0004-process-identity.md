# ADR 0004: Process Identity

Status: Accepted
Date: 2026-11-03

## Decision

Use an opaque UUIDv7 TaskId plus platform process birth identity. Never treat a
PID as a durable task identity.

Linux identity includes boot ID, PID, process start ticks, and process-group ID.
macOS identity uses `proc_pidinfo` start seconds and microseconds.

## Consequences

Restarted control planes can validate existing processes. A PID reused after a
reboot or process exit cannot match the stored identity.
