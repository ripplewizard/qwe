# QWE Development Plan

Status: Active
Updated: 2026-11-06

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

- Rust workspace with CLI, qwed supervisor, worker runtime, protocol, and
  SQLite.
- Isolated worker ownership of PTY/process groups and bounded output.
- SQLite Session/Task/Lease state, generation tombstones, and reconciliation.
- qwed-owned worker lifecycle events and a two-second safety scan.
- Long-lived Attach with snapshot continuity, resize, backpressure, and the
  interactive CLI loop.
- Persisted attachment authority, reclaim-token grace, detached holds, Lease
  release tombstones, and 24-hour durable idempotency.
- 51 workspace tests pass, including five `vt100` capability probes.

Not ready for V0:

- Fedora release acceptance, public Developer ID signing/notarization, full
  manual release acceptance, and final audit disposition remain Stage 5
  work.
- Historical cross-language POC manual disposition remains separate from the
  completed production Rust Stage 2 sign-off.

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
Status: Complete; pdsh validation passed on macOS ARM64 and Linux x86_64.

- Implement the qwed supervisor and remove the separate Host runtime path.
- Add `flock(2)` singleton protection and persistent worker control channels.
- Implement event-first worker lifecycle plus the safety-scan fallback.
- Add typed session-channel framing and machine-readable schema validation.
- Land the explicit V0 `Session 1:1 Pane 1:1 TaskInstance` storage model.
- Remove the duplicate `task.start` creation path during the clean cutover.

Exit: `just t12-host-smoke` and `just resilience-smoke` pass on macOS and Fedora
under the consolidated topology; protocol-unit and schema-drift checks pass.

Current implementation:

- qwed directly spawns and re-adopts `qwe-worker` processes; `qwe-host`, Host
  epoch registration, and 250 ms inventory polling are removed.
- A process-lifetime `flock(2)` runtime lock replaces socket-probe singleton
  behavior.
- Normal control and health probes use separate persistent worker connections.
  `SIGCHLD`/`waitpid`, control EOF, and a two-second safety scan update worker
  health without holding a global supervisor lock.
- SQLite schema v5 persists Pane identity and unique Session/Task cardinality.
  `session.create` atomically creates Session, Pane, and TaskInstance.
- `task.start` is removed from both the CLI and public protocol operation set.
- `pdsh -w localhost,istoria` runs `check-qwe`, `stage1-check`,
  `attach-e2e`, and `tui-attach-smoke` concurrently on macOS ARM64 and Linux
  x86_64; both hosts pass.

Hardening:

- `CONTROL_STATUS.reconciliation` reports current state and actual scan time.
- SQLite v6 adds Session foreign keys for Tasks, Panes, and Leases; status now
  exposes Pane records.
- `worker-loss-latency` computes P95 from 20 real worker kills.
- `safety-scan-smoke` verifies detected loss for an adopted worker that cannot
  deliver `SIGCHLD` to the current qwed.
- `pdsh -w localhost,istoria` passes `check-qwe`,
  `stage1-hardening-check`, Attach E2E, and TUI smoke on both platforms.

### Stage 2: Attach and Terminal Core

Target: 2026-11-16 to 2026-11-27
Status: Automated and independent manual acceptance passed on Linux x86_64 and
macOS ARM64.

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

Current implementation:

- Dedicated Attach sockets proxy client -> qwed -> worker. qwed now owns the
  public authority handshake and gives the worker a separate channel identity.
- Typed binary frames, raw input/output, snapshot-to-live offset continuity,
  a 1 MiB per-attachment queue, resize ordering, explicit detach, EOF release,
  complete structured VT state, and a raw-terminal CLI loop are implemented.
- `read_only`, unknown frame types, and incompatible channel versions return
  stable errors.
- `just attach-e2e` verifies channel continuity, exclusive Attach rejection,
  raw framing, resize, detach, reconnect, slow-client output backpressure, and
  the CLI loop with real sockets, processes, and PTYs.
- Stage 3 adds positive-TTL detached holds, durable reclaim-token restoration,
  generation-fenced reattach, and Lease release tombstones.
- `just tui-attach-smoke` passes a tmux-driven Neovim/htop detach and reattach
  check on both Linux x86_64 and macOS ARM64.
- Independent manual T03/T04/T08/T11 review passed on both platforms; see
  [Experiment 0010](experiments/0010-stage-2-manual-tui-signoff.md).

### Stage 3: Ownership, Recovery, and Task/Lease

Target: 2026-11-30 to 2026-12-04
Status: Complete on 2026-11-05; automated gates pass on macOS ARM64 and
Fedora Linux x86_64.

- Implement the ownership permission matrix, reclaim tokens, and grace period.
- Implement detached holds and Lease release tombstones on reattach.
- Implement `unreachable`, `lost`, orphan, and conflict reconciliation.
- Enforce durable idempotency with response replay and retention policy.
- Define Lease clock-anomaly behavior, default TTL, and history retention.
- Convert every state-matrix branch into integration tests.

Exit: authorization and liveness matrices pass on macOS and Fedora; duplicate
requests do not create duplicate Tasks; supervisor/client crashes preserve the
expected ownership and Task/Lease history.

Current implementation:

- qwed owns the persisted attachment-authority record and performs the initial
  `ATTACH_OPEN` decision before opening a worker channel.
- Reclaim tokens are 256-bit CSPRNG values; only SHA-256 hashes are persisted
  and comparisons are constant-time.
- Active authority enters a five-second grace state on supervisor restart.
- Positive-TTL detach creates a Lease-backed hold. Reattach consumes the
  current generation and writes a release tombstone before Active continues.
- Lease claim/touch/release operations validate the attachment authority
  state, and backward wall-clock movement does not shorten a renewal deadline.
- Close, task-stop, and control input require Active owner or Held
  owner+generation authority. The CLI accepts `--owner`/`--generation` and
  implements scripted input through a temporary Active channel.
- Mutating control requests require an idempotency key. Responses replay for
  24 hours across qwed restart; key reuse with a different request returns
  `idempotency_conflict`.
- Startup reconciliation probes runtime workers, reports orphan health, and
  detects closed-session, identity, alternate-socket, and duplicate-worker
  conflicts. Conflicting socket families are quarantined without deleting
  runtime evidence.
- `just stage3-check` covers the Stage 3 gate, including two-phase idempotency
  recovery and terminal-history pruning.
- `just stages-1-3-check` passes on macOS ARM64 and Fedora Linux x86_64.
- See [Experiment 0009](experiments/0009-stages-1-3-audit.md) for the Stage 1-3
  audit result and the one fixture regression found and fixed.
- See [Experiment 0012](experiments/0012-stage-3-closure.md) for the final
  closure record and reliability observation.

### Stage 4: Services, Security, and V0 Capabilities

Target: 2026-12-07 to 2026-12-18
Status: Complete on 2026-11-06.

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

Current implementation:

- ADR 0017 fixes the secret, forced-teardown, and service-asset contracts;
  ADR 0018 replaces its initial Pane resource contract.
- API Key Manager loads private credential files or macOS Keychain items into
  zeroizing memory, transfers them through a one-shot owner-only broker socket,
  and exposes descriptor 3 as `QWE_CREDENTIAL_FD` to the PTY child.
- Pane policy is persisted in SQLite v10. Linux uses one transient systemd
  cgroup v2 scope per Session for hard tree memory, swap, task-count, and CPU
  bandwidth controls. macOS uses libproc monitoring for current/peak memory,
  task count, and CPU time with explicit monitored enforcement.
- `SESSION_RESOURCE_UPDATE` changes limits on a running Active/Held Session;
  runtime policy and SQLite are rolled back together on apply failure.
- `SESSION_KILL`, `TASK_KILL_BATCH`, and confirmed `SERVER_KILL` force-kill real
  process groups and retain durable terminal history.
- Control input is limited to 1 MiB, client frames to 8 MiB, and
  `CONTROL_STATUS` reports the active capability matrix and credential backend.
- systemd and launchd crash-restart, worker adoption, and graceful stop cleanup
  pass with real service-manager jobs on both platforms.
- Managed binary-path replacement before restart preserves and re-adopts the
  live worker.
- A real second UID is denied by the private socket and rejected by qwed's
  peer-UID check after a temporary ACL bypass.
- `just stage4-check` passes on macOS ARM64 and Fedora Linux x86_64.

Local signed Keychain ACL, locked/unlocked behavior, and binary-replacement
restart pass. See
[Experiment 0020](experiments/0020-stage-4-closure.md). Stage 5 now adds
packaged installation, reinstall and upgrade recovery, release-mode
Attach/soak measurement, and strict signing/notarization tooling. Fedora release
evidence and public Developer ID trust remain pending.

### Stage 5: Release Candidate

Target: 2026-12-21 to 2026-12-24
Status: In progress on 2026-11-06; release package, installer, upgrade,
release-mode benchmark, and strict signing/notarization gate implemented on
both supported targets.

- Run release-mode Attach latency, memory, and soak benchmarks.
- Run full manual Neovim, htop, detach/attach, and job-control acceptance.
- Verify installation, upgrade, service restart, and documentation.
- Sign and notarize the macOS release artifact with the selected Developer ID
  identity and verify public-distribution trust.
- Produce final POC/manual audit disposition without rewriting failed evidence.
- Freeze V0 scope and publish an explicit supported-platform matrix.

Exit: all V0 gates pass with committed structured evidence; no open P0 design
contradiction or unsupported capability is claimed as complete.

Current implementation:

- `just release-package` builds a checksummed native archive with all four
  executables, a `qwe-release/v1` manifest, an offline-verifying installer,
  and portable native service templates.
- `just release-acceptance` installs into an isolated home, verifies versions
  and service templates, preserves SQLite across in-place reinstall, verifies
  worker adoption after supervisor restart, and migrates a disposable v9 store
  to v10.
- `just release-benchmark` exercises eight independent Sessions through the
  real Attach channel in release mode, samples aggregate RSS, and runs a
  configurable soak period.
- `packaging/notarize_macos.sh` enforces Developer ID signatures, notary
  acceptance, and Gatekeeper assessment without falling back to ad-hoc trust.
- ADR 0019 temporarily downgrades macOS binary distribution to a machine-local
  self-signed package. Local artifacts carry `-local`, report
  `public_trust: false`, and are rejected by the public notarization gate.
- `just release-gate` currently combines the workspace/state gates, packaging,
  installation/upgrade acceptance, and a 15-second release benchmark.
- macOS ARM64 and Fedora Linux x86_64 pass the package, reinstall, adoption,
  migration, and benchmark probes. macOS currently uses the local self-signed
  tier. Full manual release acceptance, public Developer ID
  signing/notarization, final POC audit disposition, and final V0 freeze remain
  open.

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

Implemented Stage gates:

```text
just protocol-check
just attach-e2e
just service-config-check
just stage4-capabilities
just stage4-check
just credential-smoke
just cross-uid-smoke
```

Still planned:

```text
just release-gate                 # remaining supported target
just release-public-gate ARTIFACT # requires selected Developer ID and profile
```

`just state-matrix`, `just release-gate`, `just release-package`,
`just release-package-self-signed`, `just macos-local-identity-create`,
`just macos-local-identity-status`, `just macos-local-identity-remove`,
`just macos-local-signing-smoke`, `just release-package-signed`,
`just release-acceptance`, `just release-benchmark`, and
`just release-public-gate` are implemented.

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
