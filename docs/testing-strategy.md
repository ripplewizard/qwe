# Testing Strategy

Status: Active
Date: 2026-11-05

## Test Layers

1. POC regression: cross-language evidence and historical comparisons.
2. Rust unit tests: IDs, protocol, storage migrations, leases, and identity.
3. Integration tests: real PTY workers, Host, qwed, SQLite, and Unix sockets.
4. Failure tests: Host/worker/qwed `SIGKILL`, stale epoch, lost Sessions.
5. Performance tests: bounded output, concurrent Attach, and latency tails.
6. Manual UX tests: Neovim, htop, detach/attach, and job control.

## Required Commands

```sh
just check-qwe
just protocol-check
just stage0-check
just stage1-check
just stage1-regression-smoke
just stage1-hardening-check
just worker-loss-latency
just safety-scan-smoke
just stage2-check
just attach-e2e
just tui-attach-smoke
just stage3-idempotency
just stage3-attach-authority
just stage3-control-authority
just stage3-reconciliation
just stage3-check
just stages-1-3-check
just t12-host-smoke
just resilience-smoke
just verify-linux-results
just verify-macos-results
```

Platform-specific verification is intentional because raw evidence stays on
the host that generated it.

## Acceptance Rules

- `pass`: automated checks, evidence, revision, and required review all agree.
- `blocked`: behavior is recorded but a required review or platform is absent.
- `fail`: observed behavior violates acceptance; do not relabel as blocked.
- `unsupported`: explicitly outside the current scope.

No aggregate score may override a failed hard gate.

## Current Performance Gates

These gates describe the consolidated supervisor topology.

- Eight concurrent clients, 64 Attach requests during continuous output.
- Zero failed requests.
- P95 at or below 3,000 ms.
- P99 at or below 5,000 ms.
- Attach journal at most 1 MiB.
- qwed plus PTY worker RSS below 200 MiB after 100 MiB output.
- Worker loss becomes `lost` within 250 ms without restarting `qwed`.
- Worker rediscovery rejects stale PID start tokens and mismatched socket paths.
- Released Lease generation never moves backwards.

Release-mode and long-duration measurements are required before V0.

## Stage 2 Attach Gate

`just attach-e2e` starts a real qwed supervisor and PTY worker, then drives the
dedicated Attach channel. It checks the handshake,
structured snapshot, exclusive-attach rejection, raw input/output framing,
stream-offset continuity, resize ordering, immediate detach, reconnect,
`read_only` rejection, unknown-frame rejection, and version rejection. It also
drives the interactive CLI through the same channel.

The gate passes on Linux x86_64 and macOS ARM64. The channel probe also opens a
slow client, drives output past the 1 MiB queue, verifies
`output_backpressure`, and reattaches to the same Session without restarting
the worker. `just tui-attach-smoke` drives real `nvim` and `htop` processes
through tmux on both platforms, detaches them, reattaches, checks the restored
screen, and exits them through the attached input channel. Independent manual
T03/T04/T08/T11 sign-off passed on both required platforms; see
[Experiment 0010](experiments/0010-stage-2-manual-tui-signoff.md).

## ADR 0011 Acceptance Gates

The supervisor and event-driven liveness implementation is covered by real
process tests. `just worker-loss-latency` runs 20 real worker kills and computes
P95; `just safety-scan-smoke` restarts qwed, re-adopts a worker, and kills that
non-child worker so only the safety scan can discover loss.

Both checks, the Stage 1 regression, T12, resilience, Attach E2E, and TUI smoke
pass under pdsh on macOS ARM64 and Fedora Linux x86_64.

- A worker exit delivered through `SIGCHLD` or control-connection EOF becomes
  `lost` with P95 at or below 50 ms.
- A deliberately dropped lifecycle event is found by the safety scan. The scan
  interval is at most 2 seconds and the fallback result is visible within
  2.5 seconds.
- The migration uses a clean pre-release runtime cutover and does not require
  compatibility with the retired Host protocol.

Reclaim-token and detached-hold acceptance remains part of Stage 3 and is not
claimed by the Stage 1 gate.

## Stage 3 Gate

`just stage3-check` runs the workspace check, existing Attach and
recovery gates, durable idempotency replay across qwed restart, and the
attachment authority probe. The authority probe covers positive-TTL detach,
wrong owner/generation rejection, Lease touch, Hold-to-Active reattach with a
release tombstone, qwed SIGKILL during Active Attach, and reclaim by the
matching token after restart.

`stage3-control-authority` covers Unattached rejection, Held owner+generation
authorization, Active owner authorization, control-input rejection outside
Active, close/task-stop behavior, and close-time Lease release.

`stage3-reconciliation` starts real qwed/qwe-worker processes and covers
healthy and unreachable orphans, closed-session conflicts, persisted identity
mismatch, duplicate workers for one SessionId, quarantine preservation, and
rejection of normal control access to unowned runtime entries.

The gate includes two-phase `SESSION_CREATE` recovery, terminal-history
pruning, and failure of pending non-creation mutations with
`idempotency_indeterminate`.

`just stages-1-3-check` runs Stage 1 hardening, Stage 2 Attach/TUI, and Stage 3
through one command. It passes on macOS ARM64 and Fedora Linux x86_64; the
audit is recorded in [Experiment 0009](experiments/0009-stages-1-3-audit.md).

## ADR 0012-0015 Contract Matrices

The corresponding ADRs are accepted. Every row has a stable ID in
[Test Matrices](test-matrices.md); those IDs are not implementation results
until the named integration test exists and passes on the required platforms.

- Attach channel: snapshot then output continuity, no duplicate or gap,
  exclusive-attach rejection, raw input/output framing, resize ordering,
  explicit detach, connection EOF, and backpressure close.
- Attach performance uses eight Sessions; same-Session exclusive contention is
  tested separately for the correct rejection or reclaim result.
- Object model: every legal Session/Task/Pane transition and rejection of
  duplicate creation or resurrection from a terminal state.
- Authorization: every cell in the ADR 0014 permission matrix, including
  active-owner, unattached, reclaim-grace, and detached-hold cases.
- Liveness and reconciliation: every row in the ADR 0015 truth table, including
  `SIGSTOP`, `SIGKILL`, missing socket, PID mismatch, orphan, duplicate, and
  Task-exited-but-worker-alive cases.
- Protocol drift: generated or checked protocol types match the accepted
  machine-readable frame and state schema before merge.

## Evidence Policy

Structured JSON and concise manual reports are committed. Raw PTY captures,
terminal snapshots, runtime SQLite databases, and environment credentials stay
local. A result marked `pass` without its referenced local evidence is invalid.
