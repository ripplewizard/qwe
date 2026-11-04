# Testing Strategy

Status: Active
Date: 2026-11-03

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

## Performance Gates

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

## Evidence Policy

Structured JSON and concise manual reports are committed. Raw PTY captures,
terminal snapshots, runtime SQLite databases, and environment credentials stay
local. A result marked `pass` without its referenced local evidence is invalid.
