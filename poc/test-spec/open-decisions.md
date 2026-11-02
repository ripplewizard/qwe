# POC Design Decisions to Freeze

These decisions are required before automated cross-language comparison starts.
Until a decision is recorded, implementations must not invent incompatible
behavior.

## P0 — Blocking

1. **Automation contract:** PTY, task lifecycle, IPC, and evidence collection
   are defined for the current slice. Detach/attach and lease commands remain.
2. **Terminal state baseline:** select a comparable VT state model and record
   which emulator/parser component provides it.
3. **Session recovery boundary:** specify separately what happens to the child
   process, PTY, shell state, terminal screen, and input stream after daemon
   restart.
4. **Task identity:** the POC uses PID plus Linux `/proc` start time or macOS
   `ps` start time. Produce a more precise macOS token before production.
5. **IPC errors:** the POC contract defines request IDs, idempotency keys, and
   JSON success/error responses for START, STATUS, and STOP.
6. **Result ownership:** structured results stay in Git candidates; raw logs
   and local environment captures remain ignored local evidence.

## Measurement Definitions

- CPU percentage convention: normalized per core or per machine.
- Memory convention: RSS, PSS, cgroup usage, and process-tree treatment.
- Startup latency boundary: process launch through first successful IPC reply.
- Recovery time boundary: daemon restart start through TaskInstance query and
  successful process re-identification.
- Long-output throughput unit: bytes, lines, and elapsed wall-clock time.

## Deferred Until Evidence Exists

- Final terminal parser or emulator choice.
- Final TaskIdentity representation beyond the POC process-start token.
- GPU attribution method when no reliable hardware association exists.
- Production CLI/TUI design.

