# ADR 0010: Explicit Product Capability Scope

Status: Accepted
Date: 2026-11-04

## Decision

The following capabilities are explicitly tracked instead of being hidden
under generic deferred scope:

- API Key Manager in D3.
- Pane CPU, memory, and process-limit policy in D3.
- Forced `kill-session`, `kill-server`, and batch Task teardown in D3.

API Key Manager must use a broker or inherited descriptor. Secrets cannot
appear in argv, environment, workspace files, logs, or persisted state.

## Consequences

The SQLite and command protocols must reserve policy fields and teardown
operations before their implementations are merged.
