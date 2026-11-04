# QWE Documentation

## Current Design

- [Design Baseline](design-baseline.md): scope, authority, recovery, leases,
  backpressure, and required capabilities.
- [Technical Architecture](technical-architecture.md): process model, crate
  graph, state ownership, persistence, and security.
- [IPC Protocol](ipc-protocol.md): field-level client, Host, and worker protocol.
- [Extension Architecture](extension-architecture.md): deferred capabilities
  and architecture reservations.
- [ADR Index](adr/README.md): accepted decisions and consequences.

## Development

- [Readiness](development-readiness.md): known gaps and start gate.
- [Roadmap](development-roadmap.md): implementation milestones.
- [Testing Strategy](testing-strategy.md): layers, commands, and acceptance.
- [Workflow](development-workflow.md): branch, commit, and evidence rules.

## Evidence

- [POC Index](poc-index.md): external POC worktree, reports, and branches.
- [Experiment 0001](experiments/0001-t12-host-ownership.md): daemon restart and
  bounded-output validation.
- [Experiment 0002](experiments/0002-resilience-and-concurrency.md): recovery,
  reconciliation, and concurrent Attach.

## Reading Order

1. Read Design Baseline.
2. Read Technical Architecture and IPC Protocol.
3. Read Readiness, Roadmap, Testing Strategy, and Workflow.
4. Read the relevant ADR before changing an accepted decision.
5. Use the POC Index only for historical evidence.
