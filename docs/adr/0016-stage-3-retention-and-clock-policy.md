# ADR 0016: Stage 3 Lease Clock, TTL, and Retention Policy

Status: Accepted
Date: 2026-11-05
Implementation state: Lease renewal, idempotency retention, and startup/daily
terminal-history pruning are implemented.

## Context

ADR 0014 defines detached holds in terms of wall-clock deadlines and monotonic
generations. Stage 3 still needs explicit limits for TTLs, behavior when the
wall clock moves, response-replay retention, and Session/Task history.

Without these limits, a call can request an effectively unbounded hold or
overwrite a future deadline when the system clock moves backward.

## Decision

- A normal control Lease uses a default TTL of 60 seconds.
- A detached hold uses a default TTL of 5 minutes.
- A client-supplied Lease or hold TTL must be positive and no greater than
  24 hours. A normal renewal may use the same bound.
- Lease deadlines remain UTC wall-clock milliseconds.
- Renewal anchors the new deadline at `max(now_ms, updated_at_ms) + ttl_ms`.
  Moving the wall clock backward must not shorten an existing deadline.
- Moving the clock forward may expire a Lease or hold. Expiry changes control
  authority only and never kills a Task or Session. The process does not
  attempt automatic clock correction.
- Idempotency scope is the logical operation and canonical payload. Successful
  results and deterministic protocol errors are replayable for 24 hours.
  Expired records are removed lazily by mutating control requests.
- An interrupted `SESSION_CREATE` is reconstructed from durable objects and
  the adopted worker. Other interrupted mutations fail closed with
  `idempotency_indeterminate`; QWE never automatically repeats an input or
  teardown effect whose outcome cannot be proven.
- Terminal Session, Pane, and TaskInstance history is retained for the later
  of 30 days or until it falls outside the newest 10,000 terminal Sessions.
  Running or otherwise non-terminal objects, and objects referenced by live
  control authority, are never pruned.
- Pruning is a bounded maintenance operation, not an implicit part of status
  reads. SQLite space reclamation is not required on every prune.

## Consequences

- Clients receive stable `invalid_request` failures for zero or excessive TTLs.
- A backward clock correction cannot create a shorter Lease generation window.
- Forward clock jumps expire control authority without destroying terminal
  work.
- Idempotency storage remains bounded by request rate and a fixed 24-hour
  horizon.
- Interrupted non-creation requests require an explicit client status check or
  a new request after the indeterminate record expires.
- History retention requires an implementation before Stage 3 can claim the
  complete Task/Lease history gate.

## Acceptance Required

- Approve the default and maximum TTLs.
- Approve backward-clock renewal anchoring and forward-clock expiry behavior.
- Approve 24-hour idempotency retention.
- Approve 30-day and 10,000-terminal-Session history retention.
