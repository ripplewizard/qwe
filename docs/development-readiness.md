# Development Readiness

Status: Stages 1-5 complete; Stage 6.5 accepted; Stages 7 and 8 complete;
Agent Execution ADR accepted
Date: 2026-11-14

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
  standalone `SESSION_RESIZE` is implemented; interactive Attach input remains
  audited-channel work.
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
  isolated-home installation, in-place reinstall, SQLite v9-to-current migration,
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
- Stage 8 slice 1 adds Pane-targeted Attach, deterministic layout query,
  concurrent exclusive channels to distinct Panes, and an interactive
  `qwe session compose` client. Freeform floating geometry, scrollback copy,
  and floating drag remain open under
  [Session Layout](stage8-session-layout.md). Slice 2 adds schema v12 and
  restart-stable Pane order, weight, z-index, and focus. Slice 3 adds relative
  split, close, and reorder controls in the API and compositor. Slice 4 adds
  focused-Pane visible-screen copy and the quick menu. Slice 5 adds fixed
  floating Panes with independent workers and persisted z-order. Slice 6 adds
  durable floating bounds and keyboard move/resize. Slice 7 adds simulated and
  live SGR mouse title dragging plus child mouse-event routing. Slice 8 adds
  bottom-right mouse resize. Slice 9 adds floating click-to-raise. Slice 10
  adds scrollback and mouse copy.

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

Linux V0 release packaging, installation, reinstall, v9-to-current migration,
systemd crash recovery, release-mode performance, TUI/job-control smoke,
historical evidence disposition, and independent manual acceptance are
complete. Public macOS distribution is deferred post-V0 under ADR 0020.

Stage 6 starts with no new runtime capability. Its open gates are clean-source
release publication, hosted core CI, a dedicated Fedora release-gate runner,
real pilot feedback, and an exercised `v0.1.x` patch release. Until those
gates close, existing `dist/` archives are local evidence rather than
published release assets.

Stage 6.5 accepts one worker per Pane, Pane-first ResourceScope, Pane-keyed
inventory and Attach, the qwed-owned observer projection, complete scope
teardown, and schema v11. Production multi-Pane behavior is not implemented by
the decision gate; SHARED-002 through SHARED-006 remain Stage 7 work.

Stage 7 is complete as Linux cgroup v2 resource governance for long-running
Sessions. It moves the V0 per-Session scope to Pane-level ResourceScope and
does not introduce a container runtime. Profiles, controller rejection,
requested/effective policy, bounded events, Pane scope teardown, schema v11,
multi-Pane inventory/recovery, RG-004 pressure, and history-scale startup and
teardown regressions pass. Fedora, Ubuntu 22.04, Ubuntu 24.04, and Debian 12
pass the installed-systemd matrix. Two consecutive six-hour 16-Session by
two-Pane runs on Ubuntu 22.04 completed eleven qwed restarts each with bounded
latency and complete cleanup, exceeding the five-hour long-run gate. The
detailed scope and evidence are in
[Stage 7 Resource Governance](stage7-resource-governance.md) and
[Experiment 0042](experiments/0042-stage7-8-closure.md).

Stage 8 is complete under
[ADR 0022](adr/0022-session-layout-compositor.md) and
[ADR 0023](adr/0023-durable-pane-layout.md). Slice 1 has deterministic
LAYOUT-001 through LAYOUT-005 coverage; LAYOUT-006 covers restart-stable layout
order and focus, while LAYOUT-007 and LAYOUT-008 cover split/close and reorder
under [ADR 0024](adr/0024-pane-composition-controls.md). LAYOUT-009 through
LAYOUT-017 cover copy, menu isolation, floating Panes, mouse move/resize,
click-to-raise, scrollback, and mouse copy. The supported macOS, Linux,
Podman, and installed-systemd matrices pass.

Agent Execution Model is accepted under
[ADR 0032](adr/0032-agent-execution-model.md). The accepted first delivery is
complete through Stage 9.5. `shell_exec`, cross-Pane orchestration beyond the
first Bubblewrap Agent Execution slice, persistent history, environment
mutation, and host management remain deferred.

Stages 9.1-9.5 provide `exec`, command handles, timeout-aware `wait_idle`,
split stdout/stderr events, bounded replay, qwed-restart recovery, worker-loss
`lost`, Pane-scope inheritance, Principal-bound hashed capabilities, ancestor
epoch checks, administrative/freeze/terminate revocation, descriptor
credentials, streaming redaction, read-only observer/push, and merged PTY
output. Interactive-shell foreground idle and explicit terminal/lost recovery
semantics are implemented.

Bubblewrap is accepted as the first external sandbox backend under
[ADR 0033](adr/0033-bubblewrap-sandbox-adapter.md). Core Rust probes pass for
FD3, Pane-cgroup inheritance, structured child identity, PTY
resize/signal/job-control behavior, and synchronized parent-death cleanup. The
capability/profile, backend failure, workspace, protected-path, home, tmp,
network, sandboxed PTY, `TIOCSTI`, FD3, and cleanup checks pass locally in
[Experiment 0049](experiments/0049-stage10-sandbox-integration.md). Sandboxed
restart recovery, worker loss, and Pane teardown pass locally. Broader
deployment-matrix qualification remains open; the container matrix currently
proves fail-closed behavior on Ubuntu 22.04, Ubuntu 24.04, and Debian 12 rather
than successful nested sandbox execution. The real-VM matrix passes the full
Stage 10 probe on Ubuntu 22.04 and Debian 12. Default Ubuntu 24.04 fails closed
under its system-wide AppArmor/userns policy; the packaged path-scoped profile
keeps that restriction enabled while passing the complete Stage 10 probe.
The explicit userns-disabled matrix passes on Ubuntu 22.04, Ubuntu 24.04, and
Debian 12, with the sandbox issue rejected and an ordinary Pane command still
completing in each environment.
Fedora 44 passes the complete Stage 10 probe while SELinux remains
`Enforcing`. Bubblewrap 0.5.0, built from the pinned source archive, also passes
the full probe on all three Ubuntu/Debian VMs, including Ubuntu 24.04 under the
packaged AppArmor profile.

Protected-path hardlink aliases are rejected before capability issue, including
hardlinks reached through a symlinked protected directory; shared PID namespace
`/proc/<pid>/root` access stays bounded, and abstract Unix sockets follow the
selected network namespace.
Protected paths and workspace cwd now use
`openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)`. Absolute protected-path
symlinks fail closed, commands inherit only expected descriptors, and workspace
FIFO credential transfer is recorded as an explicit trust-transfer channel.

The optional `strict` named seccomp profile passes pipe-split and PTY execution
and records the SHA-256 of the exact cBPF program in capability audit metadata.
Unknown named profiles fail closed.

Capability issue also verifies that `bwrap --help` contains every
production-required option. A backend that starts simple namespaces but lacks
the QWE contract fails with `bwrap_version_incompatible` before token issuance.
The oldest supported Bubblewrap behavior matrix is now covered by the full
Stage 10 probe.

Credential boundary tests now confirm both declared exfiltration channels:
inherited networking reaches a host listener, and a writable workspace retains
the FD3 credential in a host-visible file.

Landlock ABI 1+ is used as write-focused defense in depth inside mandatory
Bubblewrap confinement. The handled mask follows the detected ABI, including
`REFER`, `TRUNCATE`, and ABI 5+ device `IOCTL`. The init helper leaves reads,
execution, and private `/tmp` available, records the ABI in capability audit
metadata, and has no Landlock-only fallback path. Capability issue requires the
helper to be present and executable whenever Landlock is available.
Fedora 44 ABI 7 verification confirms `/dev` ioctls work while a char-device
node outside allowed write roots is denied.

[ADR 0034](adr/0034-sandboxed-pane-and-pty.md) now fixes the next product
direction: sandboxed Pane uses a separate capability and long-lived shim, while
interactive Agent input/resize is designed with the same terminal milestone.
Implementation and qualification remain open.
The adopted decision register and terminal output policy are frozen. D12
revocation and D25 Pane-exit semantics are resolved; D48 target-distribution
scope remains a product input. P1-P6 are complete, so implementation is no
longer blocked on architecture PoCs.
