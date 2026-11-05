# Experiment 0009: Stages 1-3 Audit

Status: Passed on macOS ARM64 and Fedora Linux x86_64
Date: 2026-11-05
Source: current `codex/stage-3-ownership-recovery` working tree

## Scope

Audit the first three production stages through the same aggregate command on
both required platforms:

```sh
just stages-1-3-check
```

The command runs Stage 1 regression, T12, resilience, worker-loss latency,
safety scan, Stage 2 protocol/Attach, the Neovim/htop TUI smoke, full Rust
format/lint/unit checks, and every Stage 3 macro/micro integration gate.

## Audit Findings Fixed

1. `worker_loss_latency.py` still sent `SESSION_CREATE` without an idempotency
   key and was correctly rejected by the Stage 3 contract. The fixture now
   sends a unique key for each request.
2. Fedora exposed a race in `SESSION_STATUS`: it returned worker state without
   applying the corresponding Task/Session transition to SQLite. Killing that
   worker shortly afterward could therefore record the Task as `lost` instead
   of preserving its observed `exited` result. Status/snapshot reads now call
   `apply_status` before returning.

Both findings were fixed before the final two-platform run.

## Final Results

| Gate | macOS ARM64 | Fedora x86_64 |
|---|---|---|
| `just stages-1-3-check` | PASS | PASS |
| Stage 1 regression | PASS | PASS |
| Worker loss P95 | 6.50 ms | 19.68 ms |
| Safety-scan detection | 2,010 ms | 2,050 ms |
| T12 qwed + worker RSS | 47,424 KiB | 37,620 KiB |
| Resilience P95 / P99 | 398 / 455 ms | 883 / 942 ms |
| Stage 2 Attach E2E | PASS | PASS |
| TUI Neovim/htop smoke | PASS | PASS |
| Stage 3 AUTH / LIVE / HIST probes | PASS | PASS |
| Idempotency and pending-create recovery | PASS | PASS |

Platforms:

- macOS 26.5.1 ARM64, Rust 1.99.1.
- Fedora release 35, Linux 5.14.10 x86_64, Rust 1.99.1.

The Fedora audit used a temporary rsync copy of the current working tree, not
the older remote `dev` checkout. Raw runtime databases and PTY captures remain
local as required by repository policy.

## Disposition

- Stage 1 automated gates: pass on both platforms.
- Stage 2 automated Attach and TUI gates: pass on both platforms. Independent
  manual TUI sign-off remains a non-blocking audit item.
- Stage 3 authorization, recovery, Task/Lease, idempotency, and retention
  gates: pass on both platforms.
