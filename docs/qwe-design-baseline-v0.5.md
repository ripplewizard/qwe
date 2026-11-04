# QWE Design Baseline v0.5

Status: Active Development Baseline
Version: 0.5
Date: 2026-11-04

## 1. Purpose

This baseline supersedes v0.4 and closes control-plane ownership, runtime
worker discovery, rediscovery identity, Lease state, and backpressure gaps.

## 2. Authority Model

| ID | Decision |
|---|---|
| D036 | `qwed` is the sole authority for SQLite Session, Task, and Lease state. |
| D037 | `qwe-host` is the authority for live worker and PTY/process health. |
| D038 | `qwed` reconciles Host inventory at startup and every 250 ms while running. |
| D039 | Worker rediscovery validates socket path, worker PID/start token, and Session ID. |
| D040 | A missing worker becomes `terminal_lost`; only that Session becomes `lost`. |

The detailed Host operations are defined in [IPC Protocol](ipc-protocol.md).

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
| D047 | Each Attach gets a private 1 MiB tail snapshot; slow clients cannot block the PTY reader. |
| D048 | Journal tail bytes are opaque diagnostics; screen state is authoritative. |
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
| `qwed` SIGKILL | Host/workers continue; startup reconciliation repairs control state |
| `qwe-host` SIGKILL | Workers continue; Host rediscovery validates identity |
| Worker SIGKILL | Runtime monitor records `lost` within 250 ms |
| Host unavailable | Clients receive explicit Host transport errors |
| OS reboot | Out of V0 scope |

## 8. Development Gates

- Automated POC evidence exists on Linux and macOS.
- Manual POC visual sign-off remains a non-blocking audit task.
- D1 adds Resize and complete VT state.
- D2 adds durable idempotency and Task control.
- D3 adds Key Manager, Pane limits, teardown, services, and security negatives.
- D4 performs release-mode soak and user-facing acceptance.
