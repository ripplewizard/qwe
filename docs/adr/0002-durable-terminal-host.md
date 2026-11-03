# ADR 0002: Durable Terminal Host

Status: Superseded in part by ADR 0006
Date: 2026-11-03

## Context

T12 showed that PTY, process, session metadata, screen state, and input are lost
when the single POC daemon is killed.

## Decision

Introduce `qwe-host` as the durable owner of PTYs, terminal state, and process
groups. `qwed` becomes a restartable control plane that discovers and
reconnects to the host.

## Consequences

- Daemon crash no longer terminates terminal sessions.
- Host protocol must be versioned independently from client IPC.
- Host crash loss was superseded by isolated workers in ADR 0006.
- Host upgrade cannot happen while sessions that require the old ABI are live.
