# QWE Design Baseline

Status: Active Development Baseline
Updated: 2026-11-04

## 1. Purpose

This is the current implementation baseline. Revisions are tracked by Git
history rather than versioned filenames.

## 2. Authority Model

| ID | Decision |
|---|---|
| D036 | `qwed` is the sole authority for SQLite Session, Task, and Lease state. |
| D037 | The V0 target is one `qwed` supervisor authoritative for the public API, SQLite, worker inventory, and control state. |
| D038 | Startup reconciliation completes before clients are accepted; runtime worker loss is event-first with a maximum 2 second safety scan. |
| D039 | Worker rediscovery validates socket path, worker PID/start token, and Session ID. |
| D040 | A missing or identity-mismatched worker becomes `lost`; a live but unresponsive worker is `unreachable` and is not declared exited. |
| D053 | Each Session remains in an independent worker process that owns its PTY, process group, VT state, and bounded journal. |

ADRs [0011](adr/0011-consolidate-control-plane.md) through
[0015](adr/0015-worker-liveness-reconciliation.md) are accepted. qwed is the
single supervisor and directly owns persistent worker control and health.

The terminal channel, object model, authorization matrix, and worker-health
contract are defined by those ADRs and represented in the
[canonical protocol schema](../protocol/schema/qwe-v1.schema.json). The current
client and worker operations are documented in [IPC Protocol](ipc-protocol.md).

## 3. Lease Semantics

| ID | Decision |
|---|---|
| D041 | Lease deadlines use UTC wall-clock milliseconds. |
| D042 | Lease owner is a logical client identity, not a PID or socket instance. |
| D043 | Expiry does not terminate Tasks or Sessions; it only removes control authority. |
| D044 | Claim/takeover requires an expired or released lease and increments generation. |
| D045 | Release writes a generation tombstone rather than deleting history. |

## 4. Output and Backpressure

| ID | Decision |
|---|---|
| D046 | The journal is per Session worker, not per client. |
| D047 | Each Attach gets a private 1 MiB journal tail and, after the Stage 2 cutover, an independent 1 MiB live-output queue. |
| D048 | Journal tail is diagnostic-only and never a replay format; screen state is authoritative. |
| D049 | Compact structured VT state with cursor and terminal modes is a D1 requirement. |

## 5. Required Product Capabilities

These capabilities are explicitly tracked rather than hidden under generic
deferred scope:

| ID | Capability | Milestone |
|---|---|---|
| D050 | API Key Manager: secrets never in argv, environment, workspace files, or logs | D3 |
| D051 | Pane resource policy: CPU, memory, and process limits attached to a Pane/Task | D3 |
| D052 | Forced teardown: `kill-session`, `kill-server`, and batch Task stop | D3 |

API Key injection must use an explicit broker or inherited file descriptor.
Secrets must be removed from the child environment before exec.

The journal may start mid-ANSI or mid-UTF-8 sequence. No feature may parse the
journal to reconstruct terminal state. Recording or diff features require a
separate gap-aware framing contract with VT state checkpoints.

## 6. Security

- Unix sockets are mode `0600`.
- Peer UID equals the server UID.
- Peer checks remain defense in depth even though owner-only sockets already
  reject other UIDs at the OS layer.
- Cross-UID, stale epoch, malformed frame, oversized input, and idempotency
  tests are required in D3.

## 7. Recovery Matrix

| Failure | Result |
|---|---|
| `qwed` SIGKILL | Workers continue; supervisor restart reconciles worker inventory |
| Worker SIGKILL | Lifecycle event records `lost`; safety scan covers missed events |
| Worker process alive but unresponsive | Health becomes `unreachable`; Session/Task are not falsely marked exited |
| Supervisor unavailable | Clients receive an explicit transport error |
| OS reboot | Out of V0 scope |

Worker exits are observed through `SIGCHLD`/`waitpid`; control-channel EOF marks
health unreachable immediately, and a two-second safety scan covers missed
events.

## 8. Development Gates

- Automated POC evidence exists on Linux and macOS.
- Manual POC visual sign-off remains a non-blocking audit task.
- Stage 0 design acceptance, stable test IDs, protocol-schema drift checking,
  macOS platform spikes, and VT capability confirmation are complete.
- D1 adds Resize and complete VT state.
- D2 adds durable idempotency and Task control.
- D3 adds Key Manager, Pane limits, teardown, services, and security negatives.
- D4 performs release-mode soak and user-facing acceptance.
