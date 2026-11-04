# Development Roadmap

Status: Active
Date: 2026-11-03

## Milestone D0: Development Baseline

- Establish `poc` as the unified evidence branch.
- Start `dev` from the validated Rust baseline.
- Keep architecture, testing, and workflow documents synchronized with code.

Exit: clean build, lint, unit tests, T12 smoke, and resilience smoke pass.

## Milestone D1: Terminal Core

- Add production `SESSION_RESIZE`.
- Complete VT cursor, modes, alternate-screen, mouse, and paste state.
- Validate TUI attach with Neovim and htop after detach and Host recovery.
- Keep journal and frame limits enforced.

Exit: visual detach/attach and resize tests pass on Linux and macOS.

## Milestone D2: Task and Lease Control

- Enforce request idempotency in durable storage.
- Complete Task stop/status/history behavior through the stable CLI protocol.
- Reconcile interrupted migrations and task exits deterministically.

Exit: duplicate requests do not create duplicate Tasks; restart preserves
Task and Lease history.

## Milestone D3: Service and Security

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

- Build the first usable CLI/TUI vertical slice.
- Run release-mode latency, memory, and soak benchmarks.
- Complete manual UX acceptance and documentation.

Exit: V0 acceptance gates in the current Design Baseline pass without
unsupported claims.
