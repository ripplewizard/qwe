# ADR 0002: Durable Terminal Host

Status: Accepted
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
- Host crash still loses live sessions in V0 and must return a distinct error.
- Host upgrade cannot happen while sessions that require the old ABI are live.
