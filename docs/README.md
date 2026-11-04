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
  consolidation; Stage 1 migration.
- [ADR 0012](adr/0012-terminal-attach-data-plane.md): accepted long-lived
  binary-framed terminal channel and snapshot handoff.
- [ADR 0013](adr/0013-core-object-model.md): accepted explicit V0 object
  cardinalities and lifecycle.
- [ADR 0014](adr/0014-attachment-ownership-authorization.md): accepted
  attachment, owner, and detached-hold authorization matrix.
- [ADR 0015](adr/0015-worker-liveness-reconciliation.md): accepted worker
  health states and startup reconciliation truth table.

## Development

- [Development Plan](development-plan.md): delivery sequence, estimates,
  gates, risks, and scope controls.
- [Readiness](development-readiness.md): known gaps and start gate.
- [Roadmap](development-roadmap.md): implementation milestones.
- [Testing Strategy](testing-strategy.md): layers, commands, and acceptance.
- [Test Matrices](test-matrices.md): stable authorization, liveness, Attach, and
  object-model case IDs.
- [Workflow](development-workflow.md): branch, commit, and evidence rules.

## Evidence

- [POC Index](poc-index.md): external POC worktree, reports, and branches.
- [Experiment 0001](experiments/0001-t12-host-ownership.md): daemon restart and
  bounded-output validation.
- [Experiment 0002](experiments/0002-resilience-and-concurrency.md): recovery,
  reconciliation, and concurrent Attach.
- [Experiment 0003](experiments/0003-stage-0-macos-capabilities.md): macOS
  Secret Provider/resource-control and VT capability probes.

## Reading Order

1. Read Design Baseline.
2. Read Technical Architecture and IPC Protocol.
3. Read Readiness, Development Plan, Roadmap, Testing Strategy, and Workflow.
4. Read the relevant ADR before changing an accepted decision.
5. Use the POC Index only for historical evidence.
