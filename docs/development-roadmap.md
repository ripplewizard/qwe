# Development Roadmap

Status: Active; Linux-first V0 complete; shared prerequisites accepted; post-V0
macOS release deferred
Date: 2026-11-09

Detailed sequencing, estimates, staffing sensitivity, and release gates are in
[Development Plan](development-plan.md).

## Milestone D0: Development Baseline

- Establish `poc` as the unified evidence branch.
- Start `dev` from the validated Rust baseline.
- Keep architecture, testing, and workflow documents synchronized with code.

Exit: clean build, lint, unit tests, T12 smoke, and resilience smoke pass.

## Milestone D1: Terminal Core

Status: Complete. The long-lived Attach channel, resize, structured VT state,
slow-client backpressure, Linux/macOS Neovim/htop detach/reattach smoke, and
independent manual T03/T04/T08/T11 review pass.

- Implement the accepted ADRs 0011-0015 contracts without reopening their
  decisions.
- Implement the long-lived terminal attach channel and snapshot handoff.
- Keep the canonical machine-readable protocol schema and drift check passing.
- Convert the ownership and worker-liveness state matrices into integration
  tests, including every rejection branch.
- Add production `SESSION_RESIZE`.
- Complete VT cursor, modes, alternate-screen, mouse, and paste state.
- Validate TUI attach with Neovim and htop after detach and Host recovery.
- Keep journal and frame limits enforced.

Exit: channel continuity, visual detach/attach, resize, and protocol-contract
tests pass on Linux and macOS.

## Milestone D2: Task and Lease Control

- Enforce request idempotency in durable storage.
- Complete Task stop/status/history behavior through the stable CLI protocol.
- Reconcile interrupted migrations and task exits deterministically.
- Define Lease clock-anomaly handling and storage-retention policy.

Exit: duplicate requests do not create duplicate Tasks; restart preserves
Task and Lease history.

## Milestone D3: Service and Security

Status: Complete. Real launchd/systemd crash-restart and stop lifecycle pass.
Linux hard cgroup scopes, macOS monitoring, dynamic policy updates, binary
replacement, local signed Keychain ACL behavior, and real second-UID rejection
pass on both platforms. Public Developer ID trust/distribution, packaged
installation, upgrade migration, and release-mode soak measurements move to
Stage 5; public macOS distribution is subsequently deferred post-V0 by ADR
0020. See ADRs 0017-0020, Experiment 0020, and `just stage4-check`.

- Add systemd user-service and launchd integration.
- Verify worker orphan cleanup and upgrade behavior.
- Add unauthorized UID, stale epoch, malformed frame, and oversized input tests.
- Add API Key Manager with broker/fd injection and secret scrubbing.
- Complete a macOS Secret Provider spike; launchd sockets are not equivalent to
  Linux credential FDs.
- Add Pane CPU, memory, and process-limit policy.
- Add `kill-session`, `kill-server`, and batch Task teardown commands.

Exit: lifecycle, negative-security, Linux credential injection, and a chosen
macOS Secret Provider design pass on both platforms.

## Milestone D4: User-Facing V0

Status: Complete for Linux-first V0. Stage 5 now has checksummed native packages, isolated
installation and in-place reinstall acceptance, SQLite v9-to-current migration,
worker adoption, release-mode eight-Session Attach/soak benchmarks, and a
strict macOS Developer ID/notarization gate. Release gates pass on macOS and
Fedora. macOS temporarily uses a local self-signed `-local` package under ADR
0019 and is paused after the Linux V0 release. Fedora passes installed-systemd
crash-restart, TUI, job-control smoke, and independent manual acceptance.
Historical evidence disposition and V0 scope freeze are complete. Public macOS
release is deferred under ADR 0020. See Experiments 0021-0025 and
`docs/release.md`.

- Build the first usable CLI/TUI vertical slice.
- Run release-mode latency, memory, and soak benchmarks.
- Complete manual UX acceptance and documentation.

Exit: V0 acceptance gates in the current Design Baseline pass without
unsupported claims.

## Milestone D5: Linux V0 Adoption and Stabilization

Status: In progress. Stage 6 formalizes release distribution, continuous
integration, feedback triage, and patch releases for the supported Linux V0
target. It does not expand the product capability boundary.

- Publish clean-source Fedora Linux x86_64 release assets and checksums.
- Require hosted core CI and a dedicated Fedora release gate.
- Run a real Linux pilot and classify reported defects by severity.
- Exercise the `v0.1.x` patch-release path.
- Keep the support matrix aligned with complete qualification evidence.

Exit: the Stage 6 exit criteria in
[Development Plan](development-plan.md#stage-6-linux-v0-adoption-and-stabilization)
pass without broadening unsupported platform or feature claims.

## Pre-D6 Gate: Shared Prerequisites

Status: Complete. Stage 6.4 passed its disposable-spike evidence and Stage 6.5
accepted [ADR 0021](adr/0021-shared-prerequisites.md) on 2026-11-09. The
accepted contract uses one worker per Pane, Pane-first ResourceScope, Pane
keyed inventory and Attach, a qwed-owned read-only observer projection, schema
v11, complete scope teardown, and the minimum Principal tombstone model.

Production qwed multi-Pane inventory, public Attach routing, observer channel
persistence, and schema v11 remain implementation work. Stage 7 is the next
track. Session Layout is the default next feature track; Agent Execution may
run in parallel only with its own accepted ADR and sufficient staffing.

Exit: satisfied. The decision is recorded in
[Development Plan](development-plan.md#stage-65-shared-prerequisites-decision-gate)
and [Storage Migration](storage-migration.md).

## Milestone D6: Linux Resource Governance

Status: Complete. Stage 7 productized Linux cgroup v2 resource controls as
Pane-first ResourceScopes for long-running Sessions. It is not a Docker or
Podman runtime. Profiles, controller validation, requested/effective policy,
bounded events, schema v11, multi-Pane inventory/recovery, RG-004 pressure,
short soak, local Fedora installed-systemd qualification, and Ubuntu
22.04/24.04 plus Debian 12 real-VM qualification pass. Two six-hour runs on
Ubuntu 22.04 completed the long-run gate with eleven qwed restarts each and
complete cleanup.

- Add reusable, opt-in resource profiles.
- Harden Pane systemd scope, controller detection, detached-process cleanup,
  and recovery.
- Expose bounded resource usage and enforcement events.
- Pass a five-hour operation-dense multi-Session resource soak.
- Validate installed systemd and cgroup behavior on each claimed Linux target.

Exit: the Stage 7 gates in
[Linux Resource Governance](stage7-resource-governance.md)
pass without claiming container or security-sandbox capabilities.

## Milestone D7: Session Layout

Status: Complete. macOS/Linux/Podman and installed-systemd qualification pass.
Stages 8.1-8.10
add Pane-targeted Attach, deterministic layout query, the client compositor,
schema v12/v13, durable layout/focus/bounds, split/close/reorder, visible and
scrollback copy, mouse selection, keyboard/mouse floating move/resize, content
mouse routing, and persisted click-to-raise z-order.

- Present multiple Pane workers through one client compositor.
- Add Pane-targeted Attach and shared tiling without changing default-Pane
  compatibility.
- Add Pane layout metadata and authorized split/close/focus controls.
- Add Pane-clipped copy and a terminal quick menu.
- Add fixed and draggable floating Panes with complete ResourceScope teardown.

Exit: the Stage 8 gates in [Session Layout](stage8-session-layout.md) pass.

## Next Feature Track: Agent Execution Model

Status: Accepted first delivery complete through Stage 9.5. The stable name for
this track is Agent Execution Model; it is not a renumbering of historical
Stage 8.

[ADR 0032](adr/0032-agent-execution-model.md) selects fresh-process `exec`,
command handles with `wait_idle`, bounded command events and replay, read-only
observation, capability and real-time revocation checks, per-command
credentials, and explicit recovery/lost states. It defers `shell_exec`,
cross-Pane orchestration, persistent history, and host management; ADR 0033
supplies the first sandbox adapter slice.

Stages 9.1-9.5 deliver `exec`, handles, wait timeout semantics, split
streams, bounded replay, Pane-scope inheritance, qwed-restart recovery, worker
loss, CLI access, Principal-bound capabilities, revocation severity, descriptor
credentials, redaction, read-only observer/push, merged PTY output, and
interactive-shell foreground idle.
See [Agent Execution Model](agent-execution-model.md).

## Next Feature Track: Bubblewrap Sandbox Adapter

Status: [ADR 0033](adr/0033-bubblewrap-sandbox-adapter.md) accepted; the
Linux `AGENT_EXEC` pipe-split and PTY slice is implemented and passes
[Experiment 0049](experiments/0049-stage10-sandbox-integration.md).

The first production slice applies workspace/network/home/tmp/PID profiles to
Linux `AGENT_EXEC`, preserves FD3 credentials and Pane cgroups, and uses
Bubblewrap `--json-status-fd`, internal `TIOCSTI` seccomp, and
`--die-with-parent`. Sandboxed restart recovery and Pane-teardown qualification
pass locally and on Ubuntu 22.04/Debian 12 real VMs. Ubuntu 24.04 currently
fails closed by default; the packaged path-scoped AppArmor profile passes the
complete matrix without disabling the global userns restriction.
[ADR 0034](adr/0034-sandboxed-pane-and-pty.md) accepts sandboxed Pane and
interactive Agent PTY as the next implementation milestone. Landlock-only
fallback, additional platform qualification, additional named seccomp profiles,
and stronger VM/OCI backends remain deferred.
The next gate is implementation. Revocation and sandboxed-Pane exit semantics
are resolved; Rocky/openEuler/Kylin scope remains a product input.
P1-P6 are complete: FD binding, shim stacking, terminal behavior, Landlock ABI
distribution, and current strict seccomp runtime compatibility are fixed.

The explicit userns-disabled matrix passes on Ubuntu 22.04, Ubuntu 24.04, and
Debian 12 and verifies ordinary Panes remain usable. SELinux-enforcing
Fedora 44 coverage passes with SELinux `Enforcing`. Bubblewrap 0.5.0 passes the
complete matrix on all three Ubuntu/Debian VMs. Additional SELinux distributions
remain open.

Escape probes now cover protected-path hardlinks, shared-PID proc roots, and
abstract Unix socket namespace isolation.

Capability issue now checks the backend's required-option help contract in
addition to namespace viability. A backend with an insufficient feature set is
rejected as `bwrap_version_incompatible` before a capability token is issued.
Credential boundary tests confirm both network exfiltration and writable
workspace persistence when the profile and command permit them.
Landlock ABI 1+ adds an optional ABI-aware write-focused layer inside mandatory
Bubblewrap confinement; it is not a standalone sandbox backend. Capability
issue fails closed if the required init helper is missing.
