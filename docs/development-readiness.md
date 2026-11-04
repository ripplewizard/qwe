# Development Readiness

Status: Ready for implementation
Date: 2026-11-04

## Decision

Rust development may start. Manual POC sign-off remains pending, but it is not
a dependency for implementation. The project must not describe manual TUI
quality or final POC audit as complete until review is signed.

Internal D1 work on PTY, VT state, and resize primitives may proceed. The public
Attach protocol and Session/Task schema must not be frozen until ADRs 0011-0015
are accepted.

## Confirmed Inputs

- Linux automated POC evidence is clean at `b42e2f9`.
- macOS automated POC evidence is clean at `46b5092`.
- All 43 matrix cases have latest results on both platforms.
- Linux-only and macOS-only verification each cover 129 expected cells.
- Cross-platform XP01 has no missing language/platform/case cell.
- Host/worker recovery, SQLite Task/Lease state, reconciliation, and concurrent
  Attach pass on macOS and Fedora.
- Host inventory reconciliation runs every 250 ms.
- Worker rediscovery validates socket path and worker start token.
- Lease release preserves a monotonic generation tombstone.

## Design Contracts Pending Acceptance

The current process topology and request/response protocol are implemented, but
the following public contracts are not frozen:

- ADR 0011: consolidate qwed and qwe-host. Not implemented.
- ADR 0012: long-lived Attach channel, typed binary frames, and snapshot
  continuity. Not implemented.
- ADR 0013: explicit V0 Session/Task/Pane cardinality and lifecycle. The
  current code still has implicit 1:1 behavior.
- ADR 0014: attachment, owner, and detached-hold authorization matrix. Not
  implemented.
- ADR 0015: worker health, unresponsive detection, and reconciliation truth
  table. Not implemented.

Public `session.attach` and Session/Task schema work must not be declared stable
until ADRs 0011-0015 are accepted and their state matrices pass.

## Known Development Gaps

These are implementation work, not reasons to delay development:

- `SESSION_RESIZE` is not implemented in the production protocol.
- Attach exposes screen text and journal tail, but not the complete cursor,
  terminal modes, alternate-screen state, mouse mode, or paste mode.
- Idempotency keys are carried by requests but are not yet deduplicated in
  durable control state.
- Idempotency response replay and retention semantics are not defined.
- Lease wall-clock anomaly handling and default detached-hold TTL are not
  defined.
- Session/Task history has no capacity or age-retention policy.
- systemd and launchd ownership of Host and worker processes is not implemented.
- Adversarial unauthorized-client and malformed-frame tests are incomplete.
- Release-mode soak and larger-concurrency measurements remain pending.
- API Key Manager, Pane resource limits, and forced teardown remain D3 scope.
- macOS Secret Provider delivery remains undecided and requires an explicit
  D3 spike before API Key Manager implementation.
- There is no machine-readable protocol schema or CI drift check comparing
  accepted ADR state matrices with `qwe-protocol`.

## Non-Blocking Audit Items

- Independent T03/T04/T08/T11 sign-off.
- T12 remains a negative historical POC result.
- Raw terminal captures and SQLite evidence stay local because repository
  policy prohibits committing them.

## Start Gate

Implementation starts from the `dev` branch. The `poc` branch is read-only
historical evidence and must not be used as the active development branch.
The executable sequence and current target dates are in the
[Development Plan](development-plan.md).
