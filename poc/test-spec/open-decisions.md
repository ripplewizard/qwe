# POC Design Decisions to Freeze

These decisions are required before automated cross-language comparison starts.
Until a decision is recorded, implementations must not invent incompatible
behavior.

## P0 — Blocking

1. **Automation contract:** define how tests create PTYs, write input, resize,
   detach/attach, start tasks, query state, and collect logs.
2. **Terminal state baseline:** select a comparable VT state model and record
   which emulator/parser component provides it.
3. **Session recovery boundary:** specify separately what happens to the child
   process, PTY, shell state, terminal screen, and input stream after daemon
   restart.
4. **Task identity:** define a stable identity beyond PID, including process
   start time or an equivalent platform mechanism.
5. **IPC errors:** define request IDs, timeouts, idempotency keys, and
   transport-versus-business error responses.
6. **Result ownership:** decide whether raw logs and binaries are retained
   locally or published as CI artifacts.

## Measurement Definitions

- CPU percentage convention: normalized per core or per machine.
- Memory convention: RSS, PSS, cgroup usage, and process-tree treatment.
- Startup latency boundary: process launch through first successful IPC reply.
- Recovery time boundary: daemon restart start through TaskInstance query and
  successful process re-identification.
- Long-output throughput unit: bytes, lines, and elapsed wall-clock time.

## Deferred Until Evidence Exists

- Final terminal parser or emulator choice.
- Final TaskIdentity representation.
- GPU attribution method when no reliable hardware association exists.
- Production CLI/TUI design.

