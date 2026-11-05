# Development Readiness

Status: Stage 1 implementation complete; dual-platform pdsh validation pending
Date: 2026-11-05

## Decision

Rust development may start. Manual POC sign-off remains pending, but it is not
a dependency for implementation. The project must not describe manual TUI
quality or final POC audit as complete until review is signed.

ADRs 0011-0015 are accepted. qwed now directly supervises workers, and the
pre-cutover Host path has been removed. The V0 runtime has one Pane and one
TaskInstance per Session, with database uniqueness constraints.

## Confirmed Inputs

- Linux automated POC evidence is clean at `b42e2f9`.
- macOS automated POC evidence is clean at `46b5092`.
- All 43 matrix cases have latest results on both platforms.
- Linux-only and macOS-only verification each cover 129 expected cells.
- Cross-platform XP01 has no missing language/platform/case cell.
- Supervisor/worker recovery, SQLite Task/Lease state, reconciliation, and
  concurrent Attach pass on macOS and Fedora.
- Worker exits are observed through `SIGCHLD`/`waitpid`; a two-second fallback
  scan uses a dedicated persistent health channel and a 500 ms PING deadline.
- Worker rediscovery validates socket path, SessionId, worker PID, and start
  token.
- Lease release preserves a monotonic generation tombstone.
- `just stage0-check`, `just check-qwe`, `just t12-host-smoke`, and
  `just resilience-smoke` pass on macOS ARM64 and Fedora Linux x86_64. The
  macOS Secret Provider/resource probe is intentionally macOS-only.

## Accepted Contracts Awaiting Implementation

The current process topology and request/response protocol are implemented, but
the following accepted contracts are not yet implemented:

- ADR 0011: qwed supervises workers directly, Host epoch/polling are removed,
  and a process-lifetime `flock(2)` protects the singleton.
- ADR 0012: the long-lived Attach channel, typed binary frames, snapshot
  continuity, resize, and an interactive CLI loop are implemented. Detached
  holds and persisted reclaim authority remain Stage 3 work.
- ADR 0013: Session 1:1 Pane 1:1 TaskInstance is persisted, `session.create` is
  the sole creation operation, and `task.start` has been removed.
- ADR 0014: attachment, owner, and detached-hold authorization matrix. Stage 3
  implementation.
- ADR 0015: event-first supervision, persistent control EOF, unreachable
  health, and the safety scan are implemented. Complete orphan/conflict state
  matrix coverage remains Stage 3.

The contract is stable; implementation status is tracked separately. A schema
entry does not claim that a runtime behavior is complete.

## Recently Closed Lifecycle Defects

The following pre-Stage-1 defects are fixed and covered by real-process
regression tests:

- Session/Task creation rollback and orphan-runtime adoption.
- Terminal SessionId/TaskId reuse.
- Premature `closed` reporting.
- Task stop closing the Session instead of preserving its final snapshot.
- Exited Session worker-loss reconciliation.
- No-op detach success; the operation now returns `unsupported_operation`.
- Stale reconciliation overwriting a newer terminal lifecycle state.

See [Experiment 0004](experiments/0004-lifecycle-regression-fixes.md).

## Known Development Gaps

These are implementation work, not reasons to delay development:

- The long-lived Attach channel implements resize, but the old request/response
  control surface still has no standalone `SESSION_RESIZE` operation.
- One-shot `SESSION_ATTACH` remains only as a diagnostic `session snapshot`
  command; its structured `terminal` field now includes cursor, modes,
  alternate-screen state, mouse mode, paste mode, and next stream offset.
- Idempotency keys are carried by requests but are not yet deduplicated in
  durable control state.
- Idempotency response replay and retention semantics are not defined.
- Lease wall-clock anomaly handling and default detached-hold TTL are not
  defined.
- Session/Task history has no capacity or age-retention policy.
- systemd and launchd ownership of supervisor and worker processes is not
  implemented.
- Adversarial unauthorized-client and malformed-frame tests are incomplete.
- Release-mode soak and larger-concurrency measurements remain pending.
- API Key Manager, Pane resource limits, and forced teardown remain D3 scope.
- macOS Secret Provider delivery has a candidate inherited-descriptor broker
  design; signed-build ACL and locked-keychain validation are due by 2026-11-13.
- Attach queue overflow is covered by a real slow-client reconnect test on both
  platforms. Concurrent independent queues cannot be observed in V0 because
  exclusive Attach rejects a second live client; queue-state isolation remains
  unit-tested.
- Positive-TTL detached holds and reclaim-token restoration are not implemented
  until Stage 3.
- Stage 2 automated Attach and tmux-driven Neovim/htop checks pass on Linux
  x86_64 and macOS ARM64. Independent manual sign-off remains pending.

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
