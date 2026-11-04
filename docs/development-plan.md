# QWE Development Plan

Status: Active
Updated: 2026-11-04

## Planning Assumptions

This plan targets V0 from the current `dev` baseline. Dates assume two Rust
engineers starting on 2026-11-05, with continuous access to macOS and Fedora
test environments. They are targets, not release commitments.

- One engineer: approximately 10-12 weeks.
- Two engineers: approximately 6-7 weeks.
- Three engineers: approximately 4-5 weeks after design acceptance.
- ADRs 0011-0015 were accepted during the Stage 0 gate, so the implementation
  critical path starts at supervisor consolidation.

The canonical protocol schema, stable contract test IDs, and initial macOS/VT
de-risking are complete.

## Current Baseline

Implemented with current repository evidence:

- Rust workspace with CLI, qwed, Host, worker runtime, protocol, and SQLite.
- Isolated worker ownership of PTY/process groups and bounded output.
- SQLite Session/Task/Lease state, generation tombstones, and reconciliation.
- qwed-to-Host epoch fencing and 250 ms inventory polling.
- One-shot Attach snapshot with 1 MiB journal tail.
- 26 workspace tests pass, including five `vt100` capability probes.

Not ready for V0:

- No long-lived terminal output channel or interactive Attach loop.
- Public resize, complete VT state, and TUI attach are incomplete.
- Session/Task/Pane and authorization contracts are accepted, but their runtime
  migration is incomplete.
- Durable idempotency, service ownership, credentials, resource limits, and
  forced teardown are incomplete.

## Critical Path

```text
Stage 0 design gate (complete)
  -> supervisor consolidation and typed protocol
  -> long-lived Attach and VT/TUI
  -> authorization, liveness, Task/Lease
  -> service/security capabilities
  -> release benchmarks and manual acceptance
```

The coupled Attach, ownership, object lifecycle, and worker-liveness decisions
are fixed. Their implementation and cross-platform state-matrix tests remain
the critical path.

## Workstreams

| Track | Scope | Can start now |
|---|---|---|
| A. Control plane | Supervisor consolidation, worker adoption, singleton lock | Interface work only |
| B. Protocol | Typed frames, Attach channel, schema checks | Frame primitives and schema design |
| C. Terminal | VT snapshot, resize, rendering, Neovim/htop behavior | Yes |
| D. State | Objects, authorization, Lease, idempotency, reconciliation | Storage analysis and tests |
| E. Platform | systemd/launchd, macOS credentials and resource control | Spike immediately |

Two-engineer sequencing should pair A+B as one stream and C+D as the other,
with Track E time-boxed inside the first two weeks.

## Delivery Schedule

### Stage 0: Design Gate and De-risking

Target: 2026-11-05 to 2026-11-06
Status: Complete on 2026-11-04.

- Accept or revise ADRs 0011-0015.
- Choose the canonical machine-readable protocol schema and CI drift check.
- Convert authorization and liveness matrices into stable test-case IDs.
- Time-box macOS Secret Provider and resource-control investigations.
- Confirm the `vt100` capability gap for cursor, modes, alternate screen,
  mouse, paste, and width handling.

Exit: no critical-path contract remains `Proposed`; platform unknowns have an
owner and a decision date.

Evidence:

- ADRs 0011-0015 are Accepted.
- The canonical schema is
  [`protocol/schema/qwe-v1.schema.json`](../protocol/schema/qwe-v1.schema.json)
  and `just protocol-check` validates schema examples, stable test IDs, frame
  codes, and Rust contract types.
- Authorization, liveness, Attach, and object-model rows have stable IDs in
  [Test Matrices](test-matrices.md).
- macOS Keychain and resource-control probes are recorded in
  [Experiment 0003](experiments/0003-stage-0-macos-capabilities.md), with owners
  and decision dates.
- `vt100` capability and width-resize limitations are covered by
  `crates/qwe-runtime/tests/vt100_capabilities.rs`.
- Stage 0, full Rust, T12, and resilience checks pass on macOS ARM64 and
  Fedora Linux x86_64; the macOS platform probe remains macOS-only.

### Stage 1: Control Plane and Protocol Foundation

Target: 2026-11-09 to 2026-11-13

- Implement the qwed supervisor and remove the separate Host runtime path.
- Add `flock(2)` singleton protection and persistent worker control channels.
- Implement event-first worker lifecycle plus the safety-scan fallback.
- Add typed session-channel framing and machine-readable schema validation.
- Land the explicit V0 `Session 1:1 Pane 1:1 TaskInstance` storage model.
- Remove the duplicate `task.start` creation path during the clean cutover.

Exit: `just t12-host-smoke` and `just resilience-smoke` pass on macOS and Fedora
under the consolidated topology; protocol-unit and schema-drift checks pass.

### Stage 2: Attach and Terminal Core

Target: 2026-11-16 to 2026-11-27

- Implement the long-lived Attach endpoint and `ATTACH_OPEN` handshake.
- Implement raw input/output framing, snapshot-to-live continuity, ACK, and
  bounded per-attachment backpressure.
- Implement production resize and ordering relative to output.
- Complete structured VT cursor, modes, alternate screen, mouse, and paste
  state.
- Build the first interactive CLI/TUI attach loop.
- Reserve but reject `read_only` mode.

Exit: shell Attach, resize, detach, and reconnect pass automatically; Neovim and
htop manual checks pass on both platforms; no output gaps or duplicates occur
in continuity tests.

### Stage 3: Ownership, Recovery, and Task/Lease

Target: 2026-11-30 to 2026-12-04

- Implement the ownership permission matrix, reclaim tokens, and grace period.
- Implement detached holds and Lease release tombstones on reattach.
- Implement `unreachable`, `lost`, orphan, and conflict reconciliation.
- Enforce durable idempotency with response replay and retention policy.
- Define Lease clock-anomaly behavior, default TTL, and history retention.
- Convert every state-matrix branch into integration tests.

Exit: authorization and liveness matrices pass on macOS and Fedora; duplicate
requests do not create duplicate Tasks; supervisor/client crashes preserve the
expected ownership and Task/Lease history.

### Stage 4: Services, Security, and V0 Capabilities

Target: 2026-12-07 to 2026-12-18

- Add systemd user-service and launchd integration.
- Verify worker orphan cleanup and supervisor upgrade behavior.
- Close unauthorized UID, malformed frame, oversized input, and replay tests.
- Implement API Key Manager using platform-approved secret injection.
- Implement macOS Secret Provider selected by the Stage 0 spike.
- Add Pane CPU, memory, and process-limit policy with explicit capability
  reporting.
- Add `kill-session`, `kill-server`, and batch Task teardown.

Exit: D3 acceptance matrix passes on both platforms. Any unsupported platform
capability is explicit and cannot be reported as implemented.

### Stage 5: Release Candidate

Target: 2026-12-21 to 2026-12-24

- Run release-mode Attach latency, memory, and soak benchmarks.
- Run full manual Neovim, htop, detach/attach, and job-control acceptance.
- Verify installation, upgrade, service restart, and documentation.
- Produce final POC/manual audit disposition without rewriting failed evidence.
- Freeze V0 scope and publish an explicit supported-platform matrix.

Exit: all V0 gates pass with committed structured evidence; no open P0 design
contradiction or unsupported capability is claimed as complete.

## Test and Delivery Gates

Every merge:

```sh
just check-qwe
```

Ownership, protocol, persistence, or concurrency changes:

```sh
just t12-host-smoke
just resilience-smoke
```

New planned Justfile gates:

```text
just protocol-check
just attach-e2e
just state-matrix
just service-smoke
just credential-smoke
just release-gate
```

The new recipes must use real processes, sockets, PTYs, SQLite, and platform
services. They must not simulate successful behavior.

## Scope Controls

Do not implement during V0:

- Multiple TaskInstances per Session.
- Read-only observers beyond the reserved Attach mode.
- Multi-node Mesh, remote scheduling, sandbox profiles, plugin systems, or
  distributed consensus.
- Unbounded terminal replay or journal parsing.

V0 may return `unsupported` for a platform capability only when the capability
matrix explicitly records it. It must not silently degrade and claim success.

## Stage 1 Entry Actions

1. Begin supervisor consolidation from a short-lived branch based on `dev`.
2. Add the `flock(2)` singleton guard and persistent worker control channels.
3. Implement event-first worker lifecycle plus the safety-scan fallback.
4. Add typed control framing and make the Stage 0 drift check mandatory for
   protocol changes.
5. Migrate storage and API behavior to explicit Session/Pane/TaskInstance 1:1
   cardinality and remove the duplicate `task.start` creation path.

## Schedule Risks

| Risk | Trigger | Mitigation |
|---|---|---|
| Late contract change | An accepted ADR is reopened without a superseding record | Require a new ADR and rerun `just protocol-check` with the state-matrix impact |
| VT engine gaps | Neovim/htop mismatch during Stage 2 | Keep engine behind trait; evaluate `alacritty_terminal` immediately |
| macOS credentials unresolved | No viable injection path by end of Stage 1 | Narrow V0 support or schedule a dedicated platform block before D3 |
| macOS resource limits diverge | No equivalent to Linux controls | Define capability matrix early; never fake enforcement |
| Supervisor migration regresses recovery | T12 or resilience smoke fails | Keep clean cutover branch and require both-platform evidence |
| Protocol/doc drift | Schema check differs from Rust types | Block merge until schema and implementation agree |
