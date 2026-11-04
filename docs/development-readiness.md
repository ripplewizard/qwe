# Development Readiness

Status: Ready for implementation
Date: 2026-11-03

## Decision

Rust development may start. Manual POC sign-off remains pending, but it is not
a dependency for implementation. The project must not describe manual TUI
quality or final POC audit as complete until review is signed.

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

## Known Development Gaps

These are implementation work, not reasons to delay development:

- `SESSION_RESIZE` is not implemented in the production protocol.
- Attach exposes screen text and journal tail, but not the complete cursor,
  terminal modes, alternate-screen state, mouse mode, or paste mode.
- Idempotency keys are carried by requests but are not yet deduplicated in
  durable control state.
- systemd and launchd ownership of Host and worker processes is not implemented.
- Adversarial unauthorized-client and malformed-frame tests are incomplete.
- Release-mode soak and larger-concurrency measurements remain pending.
- API Key Manager, Pane resource limits, and forced teardown remain D3 scope.

## Non-Blocking Audit Items

- Independent T03/T04/T08/T11 sign-off.
- T12 remains a negative historical POC result.
- Raw terminal captures and SQLite evidence stay local because repository
  policy prohibits committing them.

## Start Gate

Implementation starts from the `dev` branch. The `poc` branch is read-only
historical evidence and must not be used as the active development branch.
