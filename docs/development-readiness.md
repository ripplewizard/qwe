# Development Readiness

Status: Stages 1, 2, and 3 complete for automated acceptance
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
  concurrent Attach pass on macOS ARM64 and Fedora Linux x86_64 under the
  consolidated topology.
- Worker exits are observed through `SIGCHLD`/`waitpid`; a two-second fallback
  scan uses a dedicated persistent health channel and a 500 ms PING deadline.
- Worker rediscovery validates socket path, SessionId, worker PID, and start
  token.
- Lease release preserves a monotonic generation tombstone.
- `just stage0-check`, `just check-qwe`, `just t12-host-smoke`, and
  `just resilience-smoke` pass on macOS ARM64 and Fedora Linux x86_64. The
  macOS Secret Provider/resource probe is intentionally macOS-only.

## Accepted Contract Status

The current process topology and request/response protocol are implemented.
Contract status is:

- ADR 0011: complete. qwed supervises workers directly, Host epoch/polling are
  removed, and a process-lifetime `flock(2)` protects the singleton.
- ADR 0012: the long-lived Attach channel, typed binary frames, snapshot
  continuity, resize, and an interactive CLI loop are implemented. Stage 3 adds
  persisted reclaim authority and detached holds.
- ADR 0013: Session 1:1 Pane 1:1 TaskInstance is persisted, `session.create` is
  the sole creation operation, and `task.start` has been removed.
- ADR 0014: complete for automated acceptance on macOS and Fedora. Attach,
  close/task-stop/input, Lease operations, restart reclaim, and detached holds
  enforce the authority matrix.
- ADR 0015: event-first supervision, persistent control EOF, unreachable
  health, the safety scan, and orphan/conflict quarantine reporting are
  implemented and pass on macOS and Fedora.

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
- Mutating control requests are durably idempotent for 24 hours. A pending
  `SESSION_CREATE` reconstructs its canonical response after restart. Other
  pending mutations fail closed with `idempotency_indeterminate` instead of
  risking a duplicate effect.
- Lease renewal anchoring, maximum TTL, and 30-day/10,000-terminal-Session
  history pruning are implemented under ADR 0016.
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
- Positive-TTL detached holds, generation-fenced reattach, Lease release
  tombstones, and supervisor-restart reclaim grace are implemented.
- `TASK_STOP`, `SESSION_CLOSE`, and `SESSION_INPUT` enforce the authority matrix;
  `qwe session input` establishes a temporary Active channel automatically.
- Stage 2 automated Attach/TUI checks and independent manual
  T03/T04/T08/T11 review pass on Linux x86_64 and macOS ARM64.

## Non-Blocking Audit Items

- Historical POC T03/T04/T08/T11 evidence disposition remains open.
  Experiment 0010 signs the production Rust Stage 2 path, not the old
  cross-language POC results.
- T12 remains a negative historical POC result.
- Raw terminal captures and SQLite evidence stay local because repository
  policy prohibits committing them.

## Start Gate

Stages 0-3 automated acceptance and Stage 2 independent manual TUI sign-off are
complete. The `poc` branch is read-only historical evidence and must not be
used as the active development branch.
