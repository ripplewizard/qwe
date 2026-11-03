# QWE Documentation

## Implementation

- [Design Baseline v0.4](qwe-design-baseline-v0.4.md): current production
  scope, decisions, recovery matrix, and acceptance gates.
- [Technical Architecture](qwe-technical-architecture.md): process model, crate
  graph, IPC, state ownership, persistence, and security.
- [ADR Index](adr/README.md): accepted architecture decisions and consequences.
- [Experiment 0001](experiments/0001-t12-host-ownership.md): daemon restart and
  bounded-output validation.
- [Experiment 0002](experiments/0002-resilience-and-concurrency.md): Host crash
  recovery, control-state reconciliation, and concurrent Attach latency.

## POC Evidence

- [Technical POC Specification v0.1](qwe-technical-poc-specification-v0.1.md):
  test design and scoring method.
- [Design Baseline v0.2](qwe-design-baseline-v0.2.md): pre-implementation
  product baseline retained for history.
- [Extension Architecture Notes v0.1](qwe-extension-architecture-notes-v0.1.md):
  deferred capabilities and extension boundaries.
- [`../poc/reports/decision.md`](../poc/reports/decision.md): accepted Rust
  decision and language comparison.

## Reading Order

1. Read Baseline v0.4 for current scope and non-negotiable decisions.
2. Read Technical Architecture before adding crates, processes, or protocols.
3. Read the relevant ADR before revisiting an accepted decision.
4. Treat POC reports and v0.2 as evidence, not current implementation rules.
