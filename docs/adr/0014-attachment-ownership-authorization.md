# ADR 0014: Attachment Ownership and Authorization

Status: Accepted
Date: 2026-11-04
Implementation state: Accepted target; not implemented.

## Context

QWE currently refers to several different identities as ownership:

- The OS UID checked on a Unix socket.
- A persisted logical owner string.
- An active attachment connection.
- A detached-hold Lease with a generation.
- A reclaim token used after supervisor restart.

The documents did not define how those identities authorize individual
operations. The current implementation also uses a fixed persisted owner value,
which cannot express the accepted attachment model.

## Decision

### Identity Model

The OS UID is the V0 security principal. Owner-only sockets and same-UID peer
checks protect the transport boundary.

The logical `owner` string is a non-security identity used for attachment
arbitration, Lease fencing, and audit. It does not grant privileges beyond the
shared UID boundary and must not be treated as an authenticated remote account.

The reclaim token proves continuity of a specific attachment instance. The
Lease generation fences detached-hold changes. Neither replaces the same-UID
transport check.

### Control-Authority States

```text
Unattached
Active { owner, connection, reclaim_token_hash }
ReclaimGrace { owner, reclaim_token_hash, deadline }
Held { owner, generation, deadline }
```

Only one state applies to a Session at a time.
These states apply only to a live Session; `lost` and `closed` Sessions cannot
be attached or acquire a detached hold.

- `Unattached` grants no input, resize, teardown, or Lease mutation authority.
- `Active` is created by a successful exclusive Attach channel.
- `ReclaimGrace` exists only after supervisor restart for a persisted active
  attachment.
- `Held` is a detached hold backed by the Lease tombstone/generation model.

Client socket EOF moves `Active` to `Unattached` immediately and does not create
a detached hold. An explicit `DETACH` with a positive TTL moves it to `Held`;
without a TTL it moves directly to `Unattached`.

Leaving `Active` invalidates its reclaim token unless the transition is the
supervisor-restart move to `ReclaimGrace`.

### Permission Matrix

| Operation | Unattached | Active owner | ReclaimGrace | Held owner |
|---|---|---|---|---|
| List/status | Allow | Allow | Allow | Allow |
| Exclusive attach | Allow | Reject `already_attached` | Valid token only | Allow with generation |
| Input/resize | Reject | Allow | Reject `not_attached` | Reject `not_attached` |
| Detach | Reject | Allow | Reject | Reject |
| Close/task.stop | Reject `not_attached` | Allow | Reject | Allow with generation |
| Lease claim/takeover | Claim rules | Reject `already_attached` | Reject | Claim rules |
| Lease touch/release | Reject | Reject | Reject | Owner and generation required |

Every client attached to a different Session acts independently. This matrix
does not grant access to another Session.

Control operations that use detached-hold authority must include the current
logical owner and Lease generation. Missing or stale values return
`not_owner`.

### State Transitions

```text
Unattached --exclusive attach--> Active
Active --connection EOF--> Unattached
Active --detach without hold--> Unattached
Active --detach with TTL--> Held with new generation
Active --supervisor restart--> ReclaimGrace
ReclaimGrace --valid token--> Active
ReclaimGrace --deadline--> Unattached
Held --valid attach with generation--> Active and released Lease tombstone
Held --touch--> Held with extended deadline
Held --expiry/release--> Unattached
Held --claim/takeover--> Held under a new generation
```

`Held` does not block the Task or PTY; it only controls access to Session
control operations. Forced teardown is separately authorized in D3.

## Invariants

- At most one exclusive Active attachment exists per Session.
- Reclaim tokens are valid only in `Active` or `ReclaimGrace` for one Session.
- Reclaim tokens cannot be reused after their grace period.
- Lease generations never decrease.
- Active attachment and detached hold never apply simultaneously.
- Same-UID trust is explicit; logical owner strings do not defend against a
  malicious process running as the same OS user.

## Tests

The permission matrix and transition diagram must be converted directly into
integration cases. Every cell that can change state requires a positive test,
and every rejection requires a stable error assertion.

## Alternatives Considered

### Treat Owner as an Authentication Principal

Rejected for V0. A same-UID local process can assert any logical owner string,
so treating it as authentication would create false security.

### Keep Lease Active During Attachment

Rejected. It duplicates connection liveness with wall-clock expiry and creates
two competing authorities.

### Create a Hold on Every Disconnect

Rejected. A client crash would unnecessarily block recovery; only explicit
detach creates a detached hold.

## Consequences

- Active control and detached control have separate, testable lifecycles.
- Teardown requires explicit control authority.
- The existing fixed Session owner field must be replaced or redefined.
- Tests must distinguish transport trust from logical ownership.

## Acceptance Required

- Approve OS UID as the V0 security principal and logical owner as non-security
  fencing/audit data.
- Approve immediate release on client EOF.
- Approve the permission matrix and control-authority state machine.
