# ADR 0015: Worker Liveness and Reconciliation

Status: Accepted
Date: 2026-11-04
Implementation state: Complete for automated acceptance. Event-first
supervision, persistent-control EOF, unreachable health, the two-second safety
scan, and startup orphan/conflict dispositions pass on macOS ARM64 and Fedora
Linux x86_64.

## Context

`SIGCHLD` detects a worker that exits. A persistent control connection reports
EOF. Neither detects a worker that remains alive but cannot process requests.
The current documentation also does not define how startup reconciliation
resolves disagreement between SQLite and the runtime directory.

## Decision

Separate Session state from transient worker health:

- Session states remain those in ADR 0013.
- Worker health is `healthy`, `unreachable`, or `absent`.
- Reconciliation disposition is `matched`, `orphaned`, or `conflict`.
- `lost` is a terminal Session/Task state; `unreachable` is not.

### Detection

- A supervisor-owned worker exit is observed through `SIGCHLD` and `waitpid`.
- A re-adopted worker reports transport loss through its persistent control
  connection. EOF marks health `unreachable` immediately and schedules
  identity verification; it does not by itself mark the Task lost.
- A safety scan runs at least every 2 seconds. It validates process identity,
  socket identity, and a `PING` with a 500 ms timeout.
- One complete scan with a matching process but failed `PING` marks worker
  health `unreachable`.
- Process absence or start-token mismatch marks the worker and its running
  Session/Tasks `lost`.
- A stopped or suspended worker remains `unreachable`; QWE does not report it
  as successfully running and does not automatically kill it.

The event path remains the low-latency path. The safety scan is the fallback
for missed events, not a replacement for them.

Attach to an `unreachable` worker returns `worker_unreachable`. It does not
transfer ownership or create a detached hold.

### Reconciliation Truth Table

| SQLite | Runtime | Result |
|---|---|---|
| Running Session, matching healthy worker | Matching live identity | Keep `running` |
| Running Session, matching worker process | Worker unreachable | Keep Session/Task alive; health `unreachable` |
| Running Session, no worker process | Socket stale or absent | Mark Session/Task `lost` |
| Running Session, identity mismatch | Worker exists | Mark persisted record `lost`; quarantine runtime entry |
| No Session record | Healthy worker | Mark runtime `orphaned`; no normal Attach/input |
| No Session record | Unreachable worker | Report `orphaned` and `unreachable`; no normal Attach/input |
| Closed Session | Healthy worker with same ID | Mark `conflict`; no automatic resurrection |
| Any Session | Duplicate workers for one SessionId | Mark `conflict`; choose neither |
| Task process exited | Worker healthy | Mark Task and Session `exited`; retain final snapshot |
| Exited Session, worker unavailable | Matching process unreachable | Keep `exited`; health `unreachable` |
| Exited Session, worker absent | Stale or absent socket | Mark Session `lost`; Task remains `exited` |

Orphan and conflict discoveries are reported explicitly. They are not silently
adopted, overwritten, or deleted. A future recovery command may offer a
deliberate adoption workflow.

### Startup

The supervisor finishes the startup scan before accepting new clients. For each
record it applies the truth table, persists unambiguous transitions, and
reports ambiguous runtime entries through status. A single unreachable worker
must not block unrelated Sessions from being served.

## Invariants

- Transport failure alone never claims that a Task exited.
- Worker health never overwrites Task exit code or signal.
- Unknown runtime workers are not trusted as normal Sessions.
- Ambiguous identity never selects one worker arbitrarily.
- Runtime state is never silently deleted during reconciliation.

## Tests

The truth table must become an integration matrix covering healthy, `SIGSTOP`,
`SIGKILL`, missing socket, PID mismatch, orphan worker, duplicate worker, and
Task-exited-but-worker-alive cases on Linux and macOS.

## Alternatives Considered

### Mark Any Ping Timeout as Lost

Rejected. A suspended or temporarily overloaded worker may still own live
terminal resources, and falsely declaring loss is destructive.

### Adopt Every Worker Found in the Runtime Directory

Rejected. Stale or conflicting runtime state could resurrect a closed Session
or bind the wrong process.

### Poll Every Worker at the Old 250 ms Rate

Rejected. Event delivery handles normal loss; the scan covers missed events
without reintroducing control-plane polling as the primary authority.

## Consequences

- Unresponsive and lost become distinct user-visible conditions.
- Reconciliation needs explicit conflict/orphan reporting.
- Startup can serve healthy Sessions without waiting for unrelated workers.
- State-machine tests become mandatory cross-platform gates.

## Acceptance Required

- Approve worker health separate from Session/Task state.
- Approve event-first detection with a 2 second safety scan and 500 ms probe.
- Approve quarantine/reporting for orphan and conflict records.
