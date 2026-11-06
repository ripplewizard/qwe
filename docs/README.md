# QWE Documentation

## Current Design

- [Design Baseline](design-baseline.md): scope, authority, recovery, leases,
  backpressure, and required capabilities.
- [Technical Architecture](technical-architecture.md): process model, crate
  graph, state ownership, persistence, and security.
- [IPC Protocol](ipc-protocol.md): field-level client, Host, and worker protocol.
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

## Development

- [Development Plan](development-plan.md): delivery sequence, estimates,
  gates, risks, and scope controls.
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

## Reading Order

1. Read Design Baseline.
2. Read Technical Architecture and IPC Protocol.
3. Read Readiness, Development Plan, Roadmap, Testing Strategy, and Workflow.
4. Read the relevant ADR before changing an accepted decision.
5. Use the POC Index only for historical evidence.
