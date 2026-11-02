# Session Lease POC Contract

Status: implemented for the lease state machine, heartbeat handling, and real
PTY output isolation. In-memory persistence and attach/detach are not included.

## Scope

T09 evaluates ownership state and timer behavior only. Multi-user authorization
is outside this POC.

## States

- `owned`: one controller has an active lease.
- `idle`: no controller and the lease has not expired.
- `expired`: no controller and the lease deadline passed.

## Operations

- `LEASE_CLAIM <request-id> <session-id> <client-id>` claims an idle session.
- `LEASE_TOUCH <request-id> <session-id> <client-id>` refreshes the deadline.
- `LEASE_RELEASE <request-id> <session-id> <client-id>` enters `idle`.
- `LEASE_STATUS <request-id> <session-id>` reports owner and remaining time.
- `LEASE_TAKEOVER <request-id> <session-id> <client-id>` is accepted only after
  expiry.

The POC uses `SESSION_START <request-id> <session-id> <output-log> <program>
[args...]` to create a real PTY-backed output source for the output-is-not-input
scenario. The output pump records bytes for evidence but must not touch the
lease. Attach, detach, and interactive input remain T08 scope.

## Test Timing

The production default remains 600 seconds. Automated tests use a 300 ms lease
and verify:

1. Claim sets ownership.
2. Touch extends the deadline.
3. Release permits claim by another client.
4. A stale owner cannot touch after takeover.
5. Program output does not count as user input.
6. Client heartbeats are separate from terminal input.

All timestamps use a monotonic clock. Error responses use stable codes
`already_owned`, `not_owner`, and `not_expired`.
