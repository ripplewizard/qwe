# QWE Documentation

## Current Design

- [Design Baseline](design-baseline.md): scope, authority, recovery, leases,
  backpressure, and required capabilities.
- [Technical Architecture](technical-architecture.md): process model, crate
  graph, state ownership, persistence, and security.
- [IPC Protocol](ipc-protocol.md): field-level client, Host, and worker protocol.
- [Storage Migration](storage-migration.md): v10-to-v11 shared prerequisites,
  Pane layout through v13, command metadata in v14, capability/revocation state
  in v15, and the current v16 sandbox-profile field.
- [Protocol Schema](../protocol/schema/qwe-v1.schema.json): canonical Attach
  frames, payloads, limits, state machines, and stable test-ID manifest.
- [Extension Architecture](extension-architecture.md): deferred capabilities
  and architecture reservations.
- [ADR Index](adr/README.md): accepted and proposed decisions.
- [ADR 0011](adr/0011-consolidate-control-plane.md): accepted V0 control-plane
  consolidation; implemented qwed supervisor.
- [ADR 0012](adr/0012-terminal-attach-data-plane.md): accepted long-lived
  binary-framed terminal channel and snapshot handoff.
- [ADR 0013](adr/0013-core-object-model.md): accepted explicit V0 object
  cardinalities and lifecycle.
- [ADR 0014](adr/0014-attachment-ownership-authorization.md): accepted
  attachment, owner, and detached-hold authorization matrix.
- [ADR 0015](adr/0015-worker-liveness-reconciliation.md): accepted worker
  health states and startup reconciliation truth table.
- [ADR 0016](adr/0016-stage-3-retention-and-clock-policy.md): accepted Lease
  clock/TTL and retention policy.
- [ADR 0017](adr/0017-stage-4-service-security-and-capabilities.md): service,
  credential broker, forced teardown, and resource contracts.
- [ADR 0018](adr/0018-pane-resource-enforcement.md): Pane resource enforcement
  tiers and dynamic policy updates.
- [ADR 0019](adr/0019-temporary-macos-local-signing.md): temporary local
  self-signed macOS releases until Developer ID is available.
- [ADR 0020](adr/0020-linux-first-v0-release.md): Linux-first V0 release with
  macOS public distribution deferred to a later milestone.
- [ADR 0021](adr/0021-shared-prerequisites.md): accepted ResourceScope,
  per-Pane worker topology, channel profiles, and minimum Principal model.
- [ADR 0022](adr/0022-session-layout-compositor.md): Pane-targeted Attach,
  deterministic layout query, and the first client compositor slice.
- [ADR 0023](adr/0023-durable-pane-layout.md): schema v12 Pane ordering,
  relative weight, z-index, and durable focus.
- [ADR 0024](adr/0024-pane-composition-controls.md): relative split, close,
  focus, and reorder controls.
- [ADR 0025](adr/0025-copy-and-quick-menu.md): focused-Pane visible-screen copy
  and the client-only quick menu.
- [ADR 0026](adr/0026-fixed-floating-panes.md): worker-backed fixed floating
  Panes and persisted overlay z-order.
- [ADR 0027](adr/0027-floating-bounds-and-keyboard-move.md): schema v13 floating
  rectangles and keyboard move/resize.
- [ADR 0028](adr/0028-mouse-drag-and-hit-testing.md): SGR mouse drag,
  topmost-z hit-testing, and child mouse-event routing.
- [ADR 0029](adr/0029-mouse-resize-handle.md): floating bottom-right mouse
  resize.
- [ADR 0030](adr/0030-floating-z-order.md): floating click-to-raise and
  persisted z-order.
- [ADR 0031](adr/0031-scrollback-and-mouse-copy.md): bounded scrollback and
  focused-Pane mouse selection.
- [ADR 0032](adr/0032-agent-execution-model.md): accepted fresh-process agent
  execution, command handles, bounded events, observer, and capability model.
- [ADR 0033](adr/0033-bubblewrap-sandbox-adapter.md): accepted external
  Bubblewrap sandbox adapter, fail-closed capability profiles, and non-PTY and
  PTY Linux command integration.

## Development

- [Install](../INSTALL.md): Linux source build, package installation, service
  startup, upgrade, and uninstall.
- [Usage](../USAGE.md): CLI workflows, Sessions, Attach, Leases, resources,
  credentials, and teardown.
- [Development Plan](development-plan.md): delivery sequence, estimates,
  gates, risks, and scope controls.
- [Stage 7 Resource Governance](stage7-resource-governance.md): cgroup v2
  profiles, lifecycle hardening, observability, long-run soak, and Linux
  qualification. This is not a container-runtime plan. Current evidence is in
  [Experiment 0041](experiments/0041-linux-vm-qualification.md).
- [Stage 8 Session Layout](stage8-session-layout.md): Pane-targeted Attach,
  compositor controls, durable layout, copy/menu, and fixed floating Panes.
- [Agent Execution Model](agent-execution-model.md): Stages 9.1-9.5 and the
  completed accepted first delivery:
  fresh-process exec, bounded events, Principal capabilities, revocation,
  descriptor credentials, redaction, observer push, PTY output, and foreground
  shell idle.
- [Readiness](development-readiness.md): known gaps and start gate.
- [Roadmap](development-roadmap.md): implementation milestones.
- [Testing Strategy](testing-strategy.md): layers, commands, and acceptance.
- [Test Matrices](test-matrices.md): stable authorization, liveness, Attach,
  object-model, and idempotency case IDs.
- [Release](release.md): release gates, supported platforms, packaging, and
  public-distribution boundaries.
- [V0 Linux Manual Acceptance](v0-linux-manual-acceptance.md): final
  independent reviewer checklist and sign-off block.
- [Workflow](development-workflow.md): branch, commit, and evidence rules.
- [Stage 2 Manual TUI Review](../scripts/manual-stage2-tui/README.md):
  reproducible Fedora/macOS scripts and checklists for T03/T04/T08/T11.

## Evidence

- [POC Index](poc-index.md): external POC worktree, reports, and branches.
- [Experiment 0001](experiments/0001-t12-host-ownership.md): daemon restart and
  bounded-output validation.
- [Experiment 0002](experiments/0002-resilience-and-concurrency.md): recovery,
  reconciliation, and concurrent Attach.
- [Experiment 0003](experiments/0003-stage-0-macos-capabilities.md): macOS
  Secret Provider/resource-control and VT capability probes.
- [Experiment 0004](experiments/0004-lifecycle-regression-fixes.md): lifecycle
  transaction, teardown, identity, and reconciliation fixes on both platforms.
- [Experiment 0005](experiments/0005-stage-2-attach-channel.md): Stage 2
  Attach channel, structured VT state, resize, detach/reattach, and TUI smoke.
- [Experiment 0006](experiments/0006-stage-1-control-plane.md): qwed
  supervisor cutover, event-first worker liveness, 1:1 object model, and pdsh
  cross-platform validation.
- [Experiment 0007](experiments/0007-stage-1-hardening.md): live reconciliation,
  P95/dropped-event liveness tests, SQLite foreign keys, and Pane visibility.
- [Experiment 0008](experiments/0008-stage-3-ownership-idempotency.md): Stage 3
  attachment authority, restart reclaim, detached holds, durable idempotency,
  and terminal-history retention.
- [Experiment 0009](experiments/0009-stages-1-3-audit.md): final macOS/Fedora
  audit of Stages 1-3 and the cross-platform defects fixed by it.
- [Experiment 0010](experiments/0010-stage-2-manual-tui-signoff.md): independent
  manual T03/T04/T08/T11 sign-off on macOS and Fedora.
- [Experiment 0011](experiments/0011-kitty-keyboard-reset.md): terminal
  keyboard-protocol restoration fix and PTY regression gate.
- [Experiment 0012](experiments/0012-stage-3-closure.md): final Stage 3 exit
  evidence, reliability observation, and Stage 4 isolation.
- [Experiment 0017](experiments/0017-stage-4-core-capabilities.md): Stage 4
  credentials, resource capability, security, and forced-teardown core slice.
- [Experiment 0018](experiments/0018-pane-resource-enforcement.md): Linux hard
  cgroup limits, macOS resource monitoring, and dynamic Pane policy.
- [Experiment 0019](experiments/0019-keychain-acl-and-binary-upgrade.md): local
  signed Keychain ACL, locked-Keychain behavior, and binary replacement.
- [Experiment 0020](experiments/0020-stage-4-closure.md): final Stage 4
  cross-platform gate and second-UID rejection evidence.
- [Experiment 0021](experiments/0021-stage5-release-foundation.md): packaged
  installation, upgrade migration, and release-mode benchmark foundation.
- [Experiment 0022](experiments/0022-stage5-linux-release.md): installed
  systemd crash-restart, worker adoption, release benchmark, and TUI smoke for
  the Linux release candidate.
- [Experiment 0023](experiments/0023-v0-evidence-audit.md): final disposition
  for the historical POC T03/T04/T08/T11/T12 evidence.
- [Experiment 0024](experiments/0024-linux-oom-event-retention.md): Linux
  cgroup OOM event-retention regression and five-run verification.
- [Experiment 0025](experiments/0025-stage5-linux-v0-closure.md): Linux-first
  V0 closure, independent acceptance, and final release boundary.
- [Experiment 0026](experiments/0026-stage6-adoption-baseline.md): Stage 6
  distribution, CI, and pilot entry state.
- [Experiment 0027](experiments/0027-container-compatibility.md): rootless
  Podman smoke on Ubuntu 22.04/24.04 and Debian 12.
- [Experiment 0028](experiments/0028-stage6-shared-prerequisites-spike.md):
  per-Pane workers, resource scopes, storage migration, Attach, observer,
  recovery, and cleanup spike for ADR 0021.
- [Experiment 0029](experiments/0029-stage7-multipane-pressure-and-qualification.md):
  Stage 7 multi-Pane pressure, soak, and Linux qualification.
- [Experiment 0030](experiments/0030-stage8-layout-foundation.md): Pane-targeted
  Attach, deterministic tiling, concurrent Pane channels, and the first
  interactive compositor slice.
- [Experiment 0031](experiments/0031-stage8-durable-layout.md): schema v12
  migration, durable Pane metadata, focus persistence, and qwed restart.
- [Experiment 0032](experiments/0032-stage8-composition-controls.md):
  compositor split/close/reorder, live layout membership, and restart
  persistence.
- [Experiment 0033](experiments/0033-stage8-copy-and-menu.md): Pane-clamped copy,
  explicit clipboard results, and menu-key isolation from PTY input.
- [Experiment 0034](experiments/0034-stage8-fixed-floating-panes.md): fixed
  floating Pane rendering, focus/input, and close semantics.
- [Experiment 0035](experiments/0035-stage8-durable-floating-bounds.md):
  floating bounds persistence and keyboard move/resize.
- [Experiment 0036](experiments/0036-stage8-mouse-drag.md): simulated SGR mouse
  drag and persisted floating bounds.
- [Experiment 0037](experiments/0037-stage8-mouse-resize.md): SGR mouse resize
  handle and persisted dimensions.
- [Experiment 0038](experiments/0038-stage8-floating-z-order.md): overlapping
  floating Panes and click-to-raise persistence.
- [Experiment 0039](experiments/0039-stage8-scrollback-mouse-copy.md):
  scrollback navigation, historical copy, and mouse selection.
- [Experiment 0040](experiments/0040-stage8-cross-platform-smoke.md): native
  macOS, Linux, and rootless Podman Stage 8 smoke results.
- [Experiment 0041](experiments/0041-linux-vm-qualification.md): Ubuntu 22.04,
  Ubuntu 24.04, and Debian 12 KVM qualification plus two successful six-hour
  long-run attempts.
- [Experiment 0042](experiments/0042-stage7-8-closure.md): final Stage 7/8
  gates, interactive/observer coexistence, and Agent Execution ADR acceptance.
- [Experiment 0043](experiments/0043-stage9-agent-exec.md): Stage 9.1 real
  process exec, replay, recovery, worker-loss, and Pane-scope inheritance.
- [Experiment 0044](experiments/0044-stage9-agent-security.md): Stage 9.2
  capability scope, ancestor revocation, severity, and token redaction.
- [Experiment 0045](experiments/0045-stage9-command-credentials.md): Stage 9.3
  descriptor credentials and streaming command-output redaction.
- [Experiment 0046](experiments/0046-stage9-observer-pty.md): Stage 9.4
  read-only observer push and PTY-merged command output.
- [Experiment 0047](experiments/0047-stage9-foreground-idle.md): Stage 9.5
  interactive-shell foreground idle and first-delivery closure.
- [Experiment 0048](experiments/0048-bubblewrap-contract-poc.md): Bubblewrap
  FD3/cgroup/PTY/parent-death contract probes for ADR 0033.
- [Experiment 0049](experiments/0049-stage10-sandbox-integration.md): first
  capability-controlled Linux pipe-split and PTY sandbox execution through
  qwed, worker, `qwe-session-exec`, and Bubblewrap.

## Reading Order

1. Read Design Baseline.
2. Read Technical Architecture, Storage Migration, and IPC Protocol.
3. Read Readiness, Development Plan, Roadmap, Testing Strategy, and Workflow.
4. Read the relevant ADR before changing an accepted decision.
5. Use the POC Index only for historical evidence.
