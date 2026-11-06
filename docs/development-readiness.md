# Development Readiness

Status: Stages 1-5 complete; Stage 6 Linux adoption and stabilization in progress
Date: 2026-11-06

## Decision

Linux-first V0 development and release acceptance are complete. The historical
POC evidence audit and production Rust independent manual review are signed.
macOS remains local/experimental until a public Developer ID milestone.

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
- systemd and launchd real crash-restart, worker adoption, binary replacement,
  and graceful stop cleanup pass on Fedora Linux and macOS.
- Malformed frames, oversized frames, oversized input, and peer-UID policy are
  covered. A real second-UID client is denied by the private runtime socket and
  then rejected by the application peer-UID check after a temporary ACL bypass.
  Packaged installation and migration pass on macOS and Fedora. Public macOS
  release acceptance is deferred post-V0.
- The release package, offline installer, checksum and version checks,
  isolated-home installation, in-place reinstall, SQLite v9-to-v10 migration,
  worker adoption, and an eight-Session release-mode Attach/soak benchmark are
  implemented. macOS ARM64 and Fedora Linux x86_64 pass these probes.
- Fedora additionally passes the installed-systemd crash-restart probe,
  preserving the live worker/task after binary replacement and verifying
  closed/stopped state after service stop.
- Linux failed scopes retain `memory.events` until the worker records the
  violation and resets the scope. The OOM regression passed five consecutive
  capability runs with no remaining `qwe-session-*.scope`.
- macOS releases temporarily use a local self-signed identity under ADR 0019.
  The package is marked `local-self-signed` with `public_trust: false`; public
  binary distribution remains unavailable until Developer ID and
  notarization pass.
- API Key Manager, tree-scoped resource policy, and forced teardown are
  implemented and pass `just stage4-check` on macOS and Fedora. Linux provides
  hard cgroup v2 limits; macOS reports monitored enforcement with bounded
  polling overshoot. Local signed macOS Keychain ACL, locked-keychain failure,
  and unlock recovery pass. Public Developer ID trust is a post-V0
  release-identity requirement.
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
- Terminal detach now resets Kitty keyboard, modified-key, synchronized-output,
  paste, alternate-screen, cursor, and mouse modes; `ATTACH-015` passes on both
  platforms.

## Non-Blocking Audit Items

- Historical POC T03/T04/T08/T11 rows remain blocked and archived; Experiment
  0010 signs the separate production Rust path.
- Historical T12 remains a negative POC result; production recovery evidence
  does not rewrite it.
- The final disposition is recorded in
  [Experiment 0023](experiments/0023-v0-evidence-audit.md).
- Raw terminal captures and SQLite evidence stay local because repository
  policy prohibits committing them.

## Start Gate

Stages 0-3 automated acceptance and Stage 2 independent manual TUI sign-off are
complete. The `poc` branch is read-only historical evidence and must not be
used as the active development branch.

Stage 4 passes `just stage4-check` on macOS ARM64 and Fedora Linux x86_64,
including real launchd/systemd crash-restart, worker adoption, graceful stop
cleanup, hard Linux resource scopes, macOS resource monitoring, dynamic policy
updates, local signed Keychain ACL behavior, and real second-UID rejection.
Structured closure evidence is in
[Experiment 0020](experiments/0020-stage-4-closure.md).

Linux V0 release packaging, installation, reinstall, v9-to-v10 migration,
systemd crash recovery, release-mode performance, TUI/job-control smoke,
historical evidence disposition, and independent manual acceptance are
complete. Public macOS distribution is deferred post-V0 under ADR 0020.

Stage 6 starts with no new runtime capability. Its open gates are clean-source
release publication, hosted core CI, a dedicated Fedora release-gate runner,
real pilot feedback, and an exercised `v0.1.x` patch release. Until those
gates close, existing `dist/` archives are local evidence rather than
published release assets.
