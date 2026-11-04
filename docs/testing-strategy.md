# Testing Strategy

Status: Active
Date: 2026-11-04

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

These gates describe the implemented polling topology and remain active until
the proposed consolidation is accepted and migrated.

- Eight concurrent clients, 64 Attach requests during continuous output.
- Zero failed requests.
- P95 at or below 3,000 ms.
- P99 at or below 5,000 ms.
- Attach journal at most 1 MiB.
- Host plus PTY worker RSS below 200 MiB after 100 MiB output.
- Worker loss becomes `lost` within 250 ms without restarting `qwed`.
- Worker rediscovery rejects stale PID start tokens and mismatched socket paths.
- Released Lease generation never moves backwards.

Release-mode and long-duration measurements are required before V0.

## ADR 0011 Acceptance Gates

These gates apply only after the proposed control-plane consolidation is
implemented. They are planned acceptance criteria, not current results.

- A worker exit delivered through `SIGCHLD` or control-connection EOF becomes
  `lost` with P95 at or below 50 ms.
- A deliberately dropped lifecycle event is found by the safety scan. The scan
  interval is at most 2 seconds and the fallback result is visible within
  2.5 seconds.
- Reclaim tokens contain at least 128 CSPRNG-generated bits. SQLite stores only
  a cryptographic hash, comparison is constant-time, and the raw token never
  appears in logs or evidence.
- A valid reclaim token restores the original Session attachment during its
  per-Session grace period; invalid, stale, or mismatched tokens are rejected.
- An active grace period for one Session does not delay Attach for unaffected
  Sessions.
- After grace expiry, first-valid-Attach-wins applies unless a detached hold is
  active. A stale token gives no priority and Lease fencing remains unchanged.
- The migration uses a clean pre-release runtime cutover and does not require
  compatibility with the retired Host protocol.

## ADR 0012-0015 Planned Matrices

These are required test inputs once the corresponding ADRs are accepted. They
are not current results.

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
