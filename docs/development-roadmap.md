# Development Roadmap

Status: Active; Linux-first V0 complete; post-V0 macOS release deferred
Date: 2026-11-06

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
installation and in-place reinstall acceptance, SQLite v9-to-v10 migration,
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
