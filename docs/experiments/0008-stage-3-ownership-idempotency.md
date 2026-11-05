# Experiment 0008: Stage 3 Ownership and Durable Idempotency

Status: Passed on macOS ARM64 and Fedora Linux x86_64
Date: 2026-11-05

## Scope

This Stage 3 implementation establishes ownership, recovery, idempotency, and
retention behavior:

- Promote the accepted attachment authority model into executable policy.
- Move reclaim-token and detached-hold authority from worker memory to qwed and
  SQLite.
- Preserve ownership across supervisor restart with a bounded reclaim grace.
- Write detached holds and release tombstones with monotonic generations.
- Reject Lease claim/touch/release operations in the wrong authority state.
- Deduplicate mutating control requests and replay their original responses.

## Implemented

- `qwe-core` defines the four attachment authority states, the ADR 0014
  operation matrix, transitions, TTL defaults, and constant-time SHA-256 reclaim
  token comparison. Unit tests exercise every matrix state.
- qwed validates `ATTACH_OPEN` before opening the worker data channel, issues a
  256-bit CSPRNG token, persists only its hash, and withholds
  `ATTACH_ACCEPTED` until the durable transition commits.
- qwed intercepts `DETACH`, creates a positive-TTL hold or immediate release,
  and releases the worker channel without exposing the internal worker token.
- A detached hold writes both the authority row and Lease tombstone. Reattach
  consumes that hold through the current generation and writes a release
  tombstone before the channel becomes Active.
- qwed startup moves persisted Active authority to Reclaim Grace. Only the
  matching raw token can resume the Session before the grace deadline.
- Lease claim/takeover, touch, and release validate the attachment authority
  state. A successful claim creates a detached hold; release returns the
  authority to Unattached.
- `TASK_STOP`, `SESSION_CLOSE`, and `SESSION_INPUT` enforce Active owner or
  Held owner+generation authorization. The CLI supplies the logical owner and
  implements scripted input through a short-lived exclusive attachment.
- SQLite v8 adds attachment-authority and two-phase idempotency tables. Lease renewal
  anchors at the maximum of the current wall clock and the prior update time,
  so a backward clock move does not shorten the deadline.
- Mutating control requests require an idempotency key. Reusing a key with the
  same operation and canonical payload replays the stored response, including
  after qwed restart; a different request returns `idempotency_conflict`.
  Records expire after 24 hours.
- Idempotency records have pending and complete states. A pending
  `SESSION_CREATE` recovers a canonical response from durable objects and the
  adopted worker, completing the original record without creating another
  TaskInstance. Other interrupted mutations fail closed with
  `idempotency_indeterminate`.
- Terminal Session/Pane/Task history uses the accepted ADR 0016 retention
  policy: 30 days and newest 10,000 terminal Sessions, pruning at startup and
  daily. Attachment-authority references prevent deletion.
- Startup reconciliation probes runtime sockets, reports orphan health, and
  detects alternate-socket identity conflicts, closed-session runtime entries,
  persisted identity mismatch, and duplicate workers for one SessionId.
  Ambiguous workers are not adopted; conflicting socket families are renamed
  as evidence rather than deleted.

## Local Verification

The development host was macOS ARM64. The following commands passed:

```sh
just check-qwe
just attach-e2e
just stage3-idempotency
just stage3-attach-authority
just stage3-control-authority
just stage3-reconciliation
just stage2-check
just stage1-regression-smoke
just t12-host-smoke
just resilience-smoke
just tui-attach-smoke
```

Observed Stage 3 probe results:

- Detached hold, wrong owner, wrong generation, Lease touch, reattach, and
  release tombstone: pass.
- qwed SIGKILL after Active Attach, restart, wrong reclaim token rejection, and
  correct token resumption: pass.
- Idempotent replay in-process and after restart, key conflict, and missing-key
  rejection: pass.
- Pending `SESSION_CREATE` recovery after simulated loss of the completed
  response: pass.
- Control input/task-stop/close authority across Unattached, Held, and Active:
  pass.
- Healthy/unreachable orphan, closed conflict, identity mismatch, duplicate
  worker, exited-but-unreachable worker, and quarantine preservation: pass.
- Startup terminal-history pruning with 10,000-capacity pressure and
  attachment-authority protection: pass.
- Eight-client concurrent Attach: 64/64 requests, P95 398 ms, P99 455 ms.

These automated results passed on both required platforms. See
[Experiment 0009](0009-stages-1-3-audit.md) for the final audit.

## Remaining

- Independent manual Stage 2 TUI sign-off remains a separate audit item.
- Stage 4 service, security, resource-limit, and forced-teardown work is outside
  Stage 3.
