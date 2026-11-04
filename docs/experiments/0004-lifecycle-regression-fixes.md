# Experiment 0004: Lifecycle Regression Fixes

Status: Passed
Date: 2026-11-04
Code commit: `abb9b00`

## Scope

This pre-Stage-1 hardening change fixes lifecycle behavior that can be corrected
within the current split control-plane architecture:

- Session and Task creation now share one SQLite transaction.
- A failed persistence step compensates by closing the newly created Host
  Session; reconciliation does not adopt unknown runtime Sessions.
- A terminal SessionId cannot be reused or return an old TaskId to `running`.
- `task.stop` stops only the Task process group, moves the Session to `exited`,
  and retains the final terminal snapshot.
- `session.close` waits for the process group, escalates to `SIGKILL` after the
  grace period, and only then reports `closed`.
- An exited Session whose worker disappears becomes `lost`; its Task remains
  `exited`.
- `session detach` returns `unsupported_operation` until Stage 2 implements a
  real authority transition.
- Reconciliation cannot overwrite a newer terminal state with a stale runtime
  snapshot.

Lease authorization, attachment ownership, and durable idempotency remain
Stage 3 work. They are not claimed as complete by this experiment.

## Commands

```sh
just check-qwe
just stage0-check
just stage1-regression-smoke
just t12-host-smoke
just resilience-smoke
```

## Results

All commands passed on macOS 26.5.1 ARM64 and Fedora 35 x86_64.

The lifecycle regression suite verifies:

- Injected Task insert failure returns `persistence_failure`, the Session cannot
  be attached, and neither Session nor Task rows remain.
- Recreating a lost SessionId returns `already_exists`; the original Task
  remains exactly once with state `lost`.
- An exited Session's vanished worker produces Session `lost` and Task
  `exited`.
- `task.stop` returns `stopped`, the Session remains attachable with state
  `exited`, and the final screen snapshot is retained.
- Detach is explicitly rejected as `unsupported_operation`.
- A process group that ignores `SIGTERM` is gone when close returns `closed`.

| Platform | T12 RSS | Attach requests | P95 | P99 |
|---|---:|---:|---:|---:|
| macOS ARM64 | 45,024 KiB | 64/64 | 377 ms | 387 ms |
| Fedora x86_64 | 28,260 KiB | 64/64 | 728 ms | 830 ms |

## Remaining Limits

- Lease records still do not authorize Session operations.
- Idempotency keys are transported but not durably deduplicated.
- The long-lived Attach channel and detach state machine remain Stage 2 work.
- The qwed/qwe-host consolidation and event-first worker lifecycle remain Stage
  1 work.
