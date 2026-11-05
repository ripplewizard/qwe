# Experiment 0007: Stage 1 Hardening

Status: Passed on Linux x86_64 and macOS ARM64
Date: 2026-11-05

## Scope

This hardening closes the gaps found in the Stage 1 audit:

- Reconciliation status fields were startup snapshots presented as live data.
- Task/Pane references lacked database foreign keys.
- Pane state was not exposed by `CONTROL_STATUS`.
- Worker-loss P95 and deliberately dropped lifecycle events had no executable
  test.

## Implemented

- Reconciliation is computed at request time from persisted Session/Task state
  and current worker health. `last_scan_ms` is updated after startup and every
  safety scan.
- `CONTROL_STATUS` includes `panes`.
- SQLite v6 rebuilds Tasks, Panes, and Leases with Session foreign keys while
  retaining Task uniqueness.
- `worker-loss-latency` creates and kills 20 real workers, measures detection
  through the qwed control API, and enforces P95 at or below 50 ms.
- `safety-scan-smoke` kills the first qwed, restarts qwed around the surviving
  worker, verifies the worker is not a child of the new qwed, then kills it and
  requires the safety scan to report loss within 2.5 seconds.
- The Stage 1 regression checks live reconciliation, Pane visibility, foreign
  keys, and the existing unreachable-worker isolation behavior.

## Local Results

- `just check-qwe`: pass.
- `just stage1-hardening-check`: pass.
- `just attach-e2e`: pass.
- `just tui-attach-smoke`: pass.
- `LIVE-011`: 20 samples, P95 4.75 ms, maximum 4.79 ms.
- `LIVE-013`: adopted worker parent PID 1, detected through safety scan in
  approximately 2.0 seconds.

## pdsh Validation

The final run used:

```sh
pdsh -w localhost,istoria -f 2 \
  "just check-qwe && just stage1-hardening-check && just attach-e2e && just tui-attach-smoke"
```

| Platform | LIVE-011 P95 | LIVE-013 scan | T12 RSS | Attach P95 / P99 |
|---|---:|---:|---:|---:|
| macOS ARM64 | 5.33 ms | 2,002 ms | 46,896 KiB | 405 / 443 ms |
| Fedora Linux x86_64 | 13.32 ms | 2,019 ms | 37,152 KiB | 937 / 1,111 ms |

Both platforms passed `check-qwe`, all Stage 1 hardening gates, Attach E2E, and
the Neovim/htop smoke.

## Remaining

- Full authorization, reclaim-token, detached-hold, orphan/conflict, and
  idempotency matrix coverage remains Stage 3 work.
