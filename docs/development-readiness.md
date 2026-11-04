# Development Readiness

Status: Stage 0 complete; Stage 1 ready
Date: 2026-11-04

## Decision

Rust development may start. Manual POC sign-off remains pending, but it is not
a dependency for implementation. The project must not describe manual TUI
quality or final POC audit as complete until review is signed.

ADRs 0011-0015 are accepted. The Attach framing contract and V0 object/state
contract now have a canonical machine-readable schema and drift gate. Current
code remains on the pre-cutover migration topology until Stage 1 and Stage 2
land.

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
- `just stage0-check`, `just check-qwe`, `just t12-host-smoke`, and
  `just resilience-smoke` pass on macOS ARM64; the protocol and VT checks are
  platform-independent Rust/Python checks.

## Accepted Contracts Awaiting Implementation

The current process topology and request/response protocol are implemented, but
the following accepted contracts are not yet implemented:

- ADR 0011: consolidate qwed and qwe-host. Stage 1 migration.
- ADR 0012: long-lived Attach channel, typed binary frames, and snapshot
  continuity. Stage 2 migration.
- ADR 0013: explicit V0 Session/Task/Pane cardinality and lifecycle. The
  current code still has implicit 1:1 behavior and a duplicate creation path.
- ADR 0014: attachment, owner, and detached-hold authorization matrix. Stage 3
  implementation.
- ADR 0015: worker health, unresponsive detection, and reconciliation truth
  table. Stage 1/3 implementation.

The contract is stable; implementation status is tracked separately. A schema
entry does not claim that a runtime behavior is complete.

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
- macOS Secret Provider delivery has a candidate inherited-descriptor broker
  design; signed-build ACL and locked-keychain validation are due by 2026-11-13.
- The accepted Attach channel is not implemented even though its contract and
  test IDs are defined.

## Non-Blocking Audit Items

- Independent T03/T04/T08/T11 sign-off.
- T12 remains a negative historical POC result.
- Raw terminal captures and SQLite evidence stay local because repository
  policy prohibits committing them.

## Start Gate

Stage 0 is complete. Stage 1 starts from a short-lived branch based on `dev`.
The `poc` branch is read-only historical evidence and must not be used as the
active development branch. The executable sequence and current target dates
are in the [Development Plan](development-plan.md).
