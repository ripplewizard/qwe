# ADR 0011: Consolidate qwed and qwe-host

Status: Proposed
Date: 2026-11-04
Implementation state: Not implemented. The accepted topology in ADRs 0006 and
0009 remains authoritative until this proposal is accepted and migrated.

## Context

This ADR changes process topology only. It does not change the client protocol,
SQLite ownership model, PTY/VT requirements, or Session recovery guarantees.

The current V0 topology uses three daemon roles:

```text
qwe CLI -> qwed -> qwe-host -> session worker
```

The session worker boundary is a product requirement, not an imitation of
container infrastructure. A control-plane crash, upgrade, OOM kill, or
management mistake must not destroy every running terminal, Task process group,
and recent terminal state. Terminal work is long-lived and expensive to
reconstruct; losing all Sessions for a daemon-only failure is the primary
reliability problem QWE is designed to remove.

The separate `qwe-host` manager does not have an independent user requirement,
upgrade cadence, privilege boundary, or network failure domain. It only manages
workers; the workers themselves provide the required lifetime isolation.

Splitting control authority across qwed and Host creates:

- Host epoch registration and stale-request fencing.
- Periodic inventory reconciliation.
- Duplicate liveness authority.
- Additional IPC, compatibility, and recovery paths.

Clients connect only to qwed. Therefore, an independent Host does not preserve
client access while qwed is unavailable. It preserves only the underlying PTY
and worker lifetime, and that property is already provided by session workers.

## Decision Drivers

- Preserve live terminal work across supervisor failures.
- Keep one in-process control authority for V0.
- Prefer OS lifecycle primitives over distributed coordination.
- Preserve monotonic fencing only where detached control ownership requires it.
- Bound diagnostic data without creating a hidden replay format.

## Decision

For V0, consolidate `qwed` and `qwe-host` into one control-plane supervisor:

```text
qwe CLI -> qwed supervisor -> session worker
```

The supervisor owns the public API, SQLite, worker inventory, and control-state
transitions. Session workers remain independent and keep PTYs, process groups,
VT state, and journals alive across supervisor restarts.

Remove Host epoch registration and qwed-to-Host inventory polling. A singleton
`flock(2)` on a stable runtime lock file replaces Host epoch as the local
single-instance guard. The lock must be held for the full process lifetime and
must not depend on PID-file contents or cleanup after process exit.

## Worker Lifecycle

- Workers spawned by the supervisor are children; exit is observed through
  `SIGCHLD` and `waitpid`.
- Workers inherited after supervisor restart use a persistent control
  connection. EOF means the worker is unavailable.
- A low-frequency safety scan handles missed events and stale runtime sockets.
- On Linux, the supervisor requests `PR_SET_CHILD_SUBREAPER` so descendants
  created during its lifetime are re-parented to the supervisor rather than an
  unrelated PID 1 process.
- Subreaper status does not retroactively adopt workers already orphaned by a
  previous supervisor process.
- macOS has no equivalent subreaper guarantee; orphan tracking relies on
  worker-owned process groups, process identity, and explicit group cleanup.

Worker rediscovery must continue to validate the expected socket path, Session
ID, worker PID, and PID start token.

## Attachment Ownership

Active attachment ownership is bound to the client connection lifetime. Socket
EOF releases it immediately without heartbeat or wall-clock expiry.

Detach hold remains a persistent SQLite Lease with owner, deadline, and
monotonic generation. Expiry releases control authority only and never kills a
Task or Session.

Supervisor restart clears all active connections. To prevent an original client
from losing a race to an unrelated client during the restart window:

- The first attach returns an unguessable reclaim token.
- qwed persists only its hash plus the logical owner identity.
- On restart, a configurable short grace period gives the prior token priority.
- Clients without the prior token cannot claim during that grace period.
- The grace duration is provisional until measured; it is not a Lease.

## Journal Contract

The journal tail is a bounded diagnostic stream, not a replay format. It may
begin in the middle of an ANSI or UTF-8 sequence. Clients must not parse it to
reconstruct terminal state. Any recording, diff, or replay feature must define
a separate gap-aware format and use structured VT state as its checkpoint.

The current persisted owner string is a logical client identity. A different
owner can claim only after explicit detach or after the detached hold expires.

## Secret Provider

Linux should adapt to systemd `LoadCredential=`/credential FDs rather than
reimplement their storage and cleanup model. The secret never enters argv,
environment, workspace files, logs, or SQLite.

macOS implementation remains unresolved. launchd socket activation transfers a
listening socket, not a credential file, and Keychain may require interactive
authorization. A separate macOS credential-agent spike is required before D3;
this ADR does not claim equivalent platform support.

## Product Positioning

The design mechanism is not novel. Its value is the combination of worker
isolation, monotonic fencing generations, parsed terminal state, explicit
recovery, and local terminal semantics. Documentation must not claim that QWE
invented these individual mechanisms.

## Migration Plan

1. Record this ADR as Proposed and keep the current Design Baseline as the
   implemented state.
2. Add supervisor singleton lock and worker control-connection events.
3. Move Host inventory and worker management into qwed behind existing public
   operations.
4. Add restart attachment reclaim tokens and grace-period tests.
5. Remove Host epoch, Host registration, and periodic inventory RPC.
6. Preserve old protocol compatibility until worker/session migration completes.
7. Accept a follow-up baseline update only after Linux and macOS recovery,
   concurrency, and security tests pass.

## Acceptance Required

Adoption requires explicit decisions on all of the following:

- Merge qwed and qwe-host for V0.
- Keep each Session in an independent worker process.
- Use connection lifetime for active attachment ownership and retain a
  generation-fenced SQLite Lease only for detached holds.
- Treat the bounded journal as diagnostic-only.
- Fund a macOS Secret Provider spike before D3 implementation.

## Alternatives Considered

### Keep qwed and qwe-host Separate

Rejected for V0. It introduces epoch negotiation, periodic inventory
reconciliation, duplicate liveness authority, and additional compatibility
surface without preserving client access while qwed is unavailable.

### Embed Workers as Tokio Tasks

Rejected. Tokio tasks share the supervisor failure domain. A process crash,
upgrade, OOM event, or fatal runtime bug would destroy every Session. This
violates the primary durability requirement.

### Start with a Single-Process V0

Rejected for the current product requirement. It is the smallest implementation,
but it cannot preserve live terminal work across daemon restart. This option is
appropriate only if the product explicitly accepts tmux-style total session
loss on daemon failure.

## Consequences

- One control authority removes split-brain state and most reconciliation code.
- Worker lifetime remains independent from supervisor restarts.
- Runtime worker loss is event-driven, with a low-frequency safety scan.
- Supervisor restart needs reclaim-token protection for attachment ownership.
- Linux and macOS orphan behavior remain intentionally different and documented.
- macOS Secret Provider delivery remains an explicit open decision.
