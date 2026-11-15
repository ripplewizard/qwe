# Testing Strategy

Status: Active; Stage 7 and Stage 8 gates complete; Agent Execution ADR accepted
Date: 2026-11-14

## Test Layers

1. POC regression: cross-language evidence and historical comparisons.
2. Rust unit tests: IDs, protocol, storage migrations, leases, and identity.
3. Integration tests: real PTY workers, Host, qwed, SQLite, and Unix sockets.
4. Failure tests: Host/worker/qwed `SIGKILL`, stale epoch, lost Sessions.
5. Performance tests: bounded output, concurrent Attach, and latency tails.
6. Manual UX tests: Neovim, htop, detach/attach, and job control.

## Required Commands

```sh
just check-qwe
just protocol-check
just stage0-check
just stage1-check
just stage1-regression-smoke
just stage1-hardening-check
just worker-loss-latency
just safety-scan-smoke
just stage2-check
just attach-e2e
just terminal-reset-smoke
just tui-attach-smoke
just stage3-idempotency
just stage3-attach-authority
just stage3-control-authority
just stage3-reconciliation
just stage3-check
just stages-1-3-check
just t12-host-smoke
just resilience-smoke
just verify-linux-results
just verify-macos-results
```

Platform-specific verification is intentional because raw evidence stays on
the host that generated it.

## Acceptance Rules

- `pass`: automated checks, evidence, revision, and required review all agree.
- `blocked`: behavior is recorded but a required review or platform is absent.
- `fail`: observed behavior violates acceptance; do not relabel as blocked.
- `unsupported`: explicitly outside the current scope.

No aggregate score may override a failed hard gate.

## Current Performance Gates

These gates describe the consolidated supervisor topology.

- Eight concurrent clients, 64 Attach requests during continuous output.
- Zero failed requests.
- P95 at or below 3,000 ms.
- P99 at or below 5,000 ms.
- Attach journal at most 1 MiB.
- qwed plus PTY worker RSS below 200 MiB after 100 MiB output.
- Worker loss becomes `lost` within 250 ms without restarting `qwed`.
- Worker rediscovery rejects stale PID start tokens and mismatched socket paths.
- Released Lease generation never moves backwards.

Release-mode and long-duration measurements are required before V0.

## Stage 2 Attach Gate

`just attach-e2e` starts a real qwed supervisor and PTY worker, then drives the
dedicated Attach channel. It checks the handshake,
structured snapshot, exclusive-attach rejection, raw input/output framing,
stream-offset continuity, resize ordering, immediate detach, reconnect,
qwed-owned `read_only` observer acceptance and write rejection, unknown-frame
rejection, and version rejection. It also drives the interactive CLI through
the same channel.

The gate passes on Linux x86_64 and macOS ARM64. The channel probe also opens a
slow client, drives output past the 1 MiB queue, verifies
`output_backpressure`, and reattaches to the same Session without restarting
the worker. `just tui-attach-smoke` drives real `nvim` and `htop` processes
through tmux on both platforms, detaches them, reattaches, checks the restored
screen, and exits them through the attached input channel. Independent manual
T03/T04/T08/T11 sign-off passed on both required platforms; see
[Experiment 0010](experiments/0010-stage-2-manual-tui-signoff.md).

`just terminal-reset-smoke` verifies that CLI detach disables Kitty keyboard,
modified-key, synchronized-output, bracketed-paste, alternate-screen, cursor,
and mouse modes before returning control to the local shell.

## ADR 0011 Acceptance Gates

The supervisor and event-driven liveness implementation is covered by real
process tests. `just worker-loss-latency` runs 20 real worker kills and computes
P95; `just safety-scan-smoke` restarts qwed, re-adopts a worker, and kills that
non-child worker so only the safety scan can discover loss.

Both checks, the Stage 1 regression, T12, resilience, Attach E2E, and TUI smoke
pass under pdsh on macOS ARM64 and Fedora Linux x86_64.

- A worker exit delivered through `SIGCHLD` or control-connection EOF becomes
  `lost` with P95 at or below 50 ms.
- A deliberately dropped lifecycle event is found by the safety scan. The scan
  interval is at most 2 seconds and the fallback result is visible within
  2.5 seconds.
- The migration uses a clean pre-release runtime cutover and does not require
  compatibility with the retired Host protocol.

Reclaim-token and detached-hold acceptance remains part of Stage 3 and is not
claimed by the Stage 1 gate.

## Stage 3 Gate

`just stage3-check` runs the workspace check, existing Attach and
recovery gates, durable idempotency replay across qwed restart, and the
attachment authority probe. The authority probe covers positive-TTL detach,
wrong owner/generation rejection, Lease touch, Hold-to-Active reattach with a
release tombstone, qwed SIGKILL during Active Attach, and reclaim by the
matching token after restart.

`stage3-control-authority` covers Unattached rejection, Held owner+generation
authorization, Active owner authorization, control-input rejection outside
Active, close/task-stop behavior, and close-time Lease release.

`stage3-reconciliation` starts real qwed/qwe-worker processes and covers
healthy and unreachable orphans, closed-session conflicts, persisted identity
mismatch, duplicate workers for one SessionId, quarantine preservation, and
rejection of normal control access to unowned runtime entries.

The gate includes two-phase `SESSION_CREATE` recovery, terminal-history
pruning, and failure of pending non-creation mutations with
`idempotency_indeterminate`.

`just stages-1-3-check` runs Stage 1 hardening, Stage 2 Attach/TUI, and Stage 3
through one command. It passes on macOS ARM64 and Fedora Linux x86_64; the
audit is recorded in [Experiment 0009](experiments/0009-stages-1-3-audit.md).

## Stage 4 Gate

`just stage4-check` runs the workspace gate, service-manifest validation, a
real launchd/systemd lifecycle probe, the Stage 4 capability probe, a real
second-UID socket probe, and durable idempotency recovery. The probes start
qwed, workers, PTYs, and child processes against real control sockets and
SQLite.

It covers:

- Private credential files, symlink/traversal/oversize rejection, inherited
  descriptor delivery, and secret redaction from argv, environment, status,
  terminal output, logs, and SQLite.
- Aggregate CPU-time enforcement, Linux cgroup v2 hard memory/swap/task/CPU
  bandwidth controls, macOS libproc monitoring, current/peak metrics, and
  explicit unsupported-tier responses.
- Malformed and oversized control frames, oversized input, and the peer-UID
  rejection policy.
- Session, batch Task, and server forced teardown in the process table and
  durable state.
- Dynamic resource-policy updates on a running Held Session, including runtime
  application and durable Pane policy verification.
- systemd unit structure, launchd plist linting, managed SIGKILL restart,
  worker adoption by the restarted qwed, graceful SIGTERM cleanup, and persisted
  terminal Session/Task state.
- Binary path replacement before a managed restart, plus local signed Keychain
  ACL, locked-Keychain fail-closed, and unlock recovery on macOS.
- A second UID is denied by the private runtime socket; after a temporary ACL
  bypass, qwed still rejects the connected peer through its application-level
  UID check.

The gate passes on macOS ARM64 and Fedora Linux x86_64. Public Developer ID
trust/distribution remains Stage 5 release packaging.

`just cross-uid-smoke` verifies an isolated runtime directory with the same
owner-only directory and socket semantics used by the service assets. It
requires the unprivileged `qwe-probe` account and one narrow sudoers rule
documented in `packaging/README.md`. It does not install or package QWE;
packaged installation is covered by the Stage 5 release gate below.

## Stage 5 Release Gate

`just release-gate` combines the workspace and persisted state-matrix gates
with a release profile build, native package, isolated installation, upgrade
recovery, and release-mode performance probe.

The release probes exercise:

- SHA-256 package verification and version/protocol reporting for all four
  executables.
- Installation into an isolated home without modifying service-manager state
  or SQLite.
- Native service-template rendering with no checkout-path dependency.
- In-place reinstall, SQLite preservation, qwed crash/restart, and worker/task
  adoption through the installed sibling executables.
- SQLite v9-to-current startup migration without resetting state.
- Eight independent Sessions, real Attach handshakes and snapshots, aggregate
  RSS sampling, and a configurable sustained soak.
- On Linux, the installed systemd unit, binary reinstall, supervisor
  crash-restart, worker/task adoption, graceful persistence, Neovim/htop, and
  Ctrl-Z/jobs/bg/fg job control.

Linux-first V0 is defined by ADR 0020. Independent Linux manual acceptance is
the final human gate. Public macOS distribution is a separate post-V0 strict
gate: Developer ID signing, notary acceptance, and Gatekeeper assessment must
pass; a local certificate or ad-hoc signature is not accepted as public trust.

Stage 6 adds two continuous feedback loops. Hosted Linux and macOS jobs repeat
formatting, Clippy, workspace tests, protocol validation, and release builds on
every integration change. A dedicated Fedora x86_64 runner separately repeats
the complete `just release-gate`. The hosted jobs prove portability of the
source checks; only the Fedora runner can produce supported V0 release
evidence.

`just container-smoke` runs the same real Session/input/snapshot/restart smoke
inside rootless Podman images. It is a compatibility probe, not a substitute
for installed-systemd, cgroup, migration, or manual release acceptance.

Stage 7 extends resource testing around the accepted Pane-first ResourceScope:
controller availability, profile validation, Pane scope startup and cleanup
under all crash paths, detached-job teardown, bounded enforcement events,
8/16-Session pressure runs, and a five-hour operation-dense qualification on real
installed-systemd Linux targets. SHARED-002 through SHARED-006 in the
[Test Matrices](test-matrices.md) cover multi-Pane inventory, Pane-addressed
Attach, schema v11, observer authorization, and complete scope cleanup.
Container compatibility tests cannot substitute for these gates.

The executable Stage 7 gates are `stage7-pressure`, `stage7-multi-pane`,
`qwed-restart-volume`, `stage7-reconciliation-scale`, `stage7-teardown-scale`,
`stage7-soak-short`, `stage7-soak`,
`stage7-qualification-node`, and `stage7-qualification-matrix`. Qualification
output records distribution and VM reachability explicitly; unavailable
targets are `blocked`, never `pass`.

Until Developer ID is available, ADR 0019 permits a dedicated local
self-signed package tier. It signs all four executables through an isolated
Keychain, records `local-self-signed` and `public_trust: false`, adds the
`-local` package suffix, and is rejected by the public gate before any Apple
notary submission. `just macos-local-signing-smoke` separately verifies the
create, sign, inspect, remove, and Keychain-search-list restoration lifecycle.

Stage 8 slice 1 adds `just stage8-layout`. It creates a two-Pane Session and
checks deterministic tiling, Pane-targeted Attach, concurrent exclusive
channels to distinct Panes, focused input, compositor rendering, focus
switching, and terminal reset on detach. LAYOUT-006 additionally restarts qwed
and verifies durable layout order and focus. LAYOUT-007 and LAYOUT-008 exercise
relative split/close and focus-preserving reorder through the interactive
compositor. LAYOUT-009 and LAYOUT-010 verify focused-Pane visible-screen copy
with an explicit clipboard provider and confirm menu keys never reach the PTY.
LAYOUT-011 creates a worker-backed fixed floating Pane, verifies overlay focus
and input, and closes it through normal teardown. LAYOUT-012 verifies keyboard
move/resize and persisted floating bounds. LAYOUT-013 injects SGR mouse
press/motion/release into a real compositor PTY and verifies title-row dragging.
LAYOUT-014 drags the floating bottom-right resize handle and verifies persisted
dimensions. LAYOUT-015 verifies click-to-raise on overlapping floating Panes.
LAYOUT-016 verifies bounded scrollback copy and LAYOUT-017 verifies mouse
selection in copy mode.

Stages 9.1-9.5 add `just stage9-agent-exec`. It starts explicit non-PTY and PTY
commands in a real Pane worker and verifies exit codes, split stdout/stderr,
signals, spawn failure, deadlines, wait timeout, bounded replay gaps, CLI
access, qwed restart, Pane-cgroup inheritance, explicit `lost` after worker
loss, capability scope,
expiry and ancestor invalidation, administrative preservation, freeze
escalation, low-trust denial, token non-persistence, descriptor credential
delivery, split-chunk redaction, credential failure closure, read-only
multi-frame observer push, merged PTY output, and interactive-shell foreground
idle. The recipe is the current evidence for AGENT-001 through AGENT-012.

ADR 0033 adds `just bubblewrap-poc`, a Linux-only Rust contract test suite. It
skips when Bubblewrap is unavailable and currently covers SANDBOX-001 through
SANDBOX-003: FD3/cgroup/structured child PID, PTY resize/signal/job-control
semantics, and synchronized `--die-with-parent`. `just stage10-sandbox` covers
SANDBOX-004 through SANDBOX-012 through qwed, worker, and Bubblewrap, including
capability authorization, backend failure, workspace, protected paths, home,
tmp, network policy, sandboxed PTY, `TIOCSTI` denial, qwed restart, worker loss,
Pane teardown, the content-hashed `strict` named seccomp denylist, and rejection
of a backend missing a production-required option. It explicitly confirms that
a writable workspace can persist a credential and reports the corresponding
risk. It also rejects protected-path hardlink aliases, verifies
`/proc/<pid>/root` cannot escape the mount policy, and checks abstract Unix
socket isolation. `just landlock-poc` separately verifies write confinement and
read/private tmp preservation; Stage 10 confirms the ABI is applied through the
full Bubblewrap process chain and fails capability issue when the init helper
is missing. It also verifies that a backend failure followed by repair is
observed by the next capability issue without restarting qwed or the worker.
`just sandbox-container-matrix` verifies fail-closed behavior on Ubuntu 22.04,
Ubuntu 24.04, and Debian 12 when Podman disallows Bubblewrap's nested mount/PID
setup.

`just sandbox-vm-matrix` uses the existing rootless QEMU lab. Ubuntu 22.04 and
Debian 12 pass the complete Stage 10 probe. Default Ubuntu 24.04 rejects user
ID mapping under the system-wide AppArmor/userns policy, and qwed must fail
closed.
`just sandbox-apparmor-vm` installs the packaged path-scoped Bubblewrap profile
on Ubuntu 24.04, verifies the global restriction remains enabled, and passes
the complete Stage 10 probe.
`just sandbox-landlock-vm ubuntu2404` runs the init directly against Landlock
ABI 4 and verifies `REFER` rename denial and `TRUNCATE` denial outside allowed
write roots.
The same recipe on Fedora 44 verifies Landlock ABI 7 `IOCTL_DEV`: `/dev/null`
ioctls remain usable and a char-device node outside allowed write roots is
denied.
`just sandbox-selinux-vm` runs the complete probe on Fedora 44 and requires
SELinux to remain `Enforcing`; the externally managed VM target and SSH config
are supplied with `QWE_SELINUX_SSH_TARGET` and `QWE_SELINUX_SSH_CONFIG`.
`just sandbox-bubblewrap-oldest-vm-matrix`
builds the pinned Bubblewrap 0.5.0 source and runs the complete probe on
Ubuntu 22.04, Ubuntu 24.04 with AppArmor, and Debian 12.
`just sandbox-userns-disabled-vm-matrix` temporarily disables userns on Ubuntu
22.04, Ubuntu 24.04, and Debian 12, requires classified `userns_disabled`,
restores sysctls afterward, and verifies an ordinary Pane command still
completes.
`just sandbox-systemd-properties` verifies Bubblewrap RW workspace mounts under
the packaged user-service hardening properties.
`just sandbox-systemd-service-smoke` exercises the full qwed service lifecycle:
sandbox Agent execution, crash recovery, binary replacement, restart, and
teardown.

## ADR 0012-0015 Contract Matrices

The corresponding ADRs are accepted. Every row has a stable ID in
[Test Matrices](test-matrices.md); those IDs are not implementation results
until the named integration test exists and passes on the required platforms.

- Attach channel: snapshot then output continuity, no duplicate or gap,
  exclusive-attach rejection, raw input/output framing, resize ordering,
  explicit detach, connection EOF, and backpressure close.
- Attach performance uses eight Sessions; same-Session exclusive contention is
  tested separately for the correct rejection or reclaim result.
- Object model: every legal Session/Task/Pane transition and rejection of
  duplicate creation or resurrection from a terminal state.
- Authorization: every cell in the ADR 0014 permission matrix, including
  active-owner, unattached, reclaim-grace, and detached-hold cases.
- Liveness and reconciliation: every row in the ADR 0015 truth table, including
  `SIGSTOP`, `SIGKILL`, missing socket, PID mismatch, orphan, duplicate, and
  Task-exited-but-worker-alive cases.
- Protocol drift: generated or checked protocol types match the accepted
  machine-readable frame and state schema before merge.

## Evidence Policy

Structured JSON and concise manual reports are committed. Raw PTY captures,
terminal snapshots, runtime SQLite databases, and environment credentials stay
local. A result marked `pass` without its referenced local evidence is invalid.
