# V0 Contract Test Matrices

Status: Stages 1-3 automated matrices validated on macOS ARM64 and Fedora Linux
x86_64
Date: 2026-11-05

These IDs are stable contract identifiers. An implementation test may be
named after an ID only after it exercises the complete row with real sockets,
processes, PTYs, and SQLite where applicable. The IDs are also listed in the
canonical [protocol schema](../protocol/schema/qwe-v1.schema.json); changing
either copy requires passing `just protocol-check`.

## Attachment Authorization

States are mutually exclusive and apply only to a live Session.

| ID | Operation | Authority state | Expected result |
|---|---|---|---|
| AUTH-001 | List/status | Unattached | Allow |
| AUTH-002 | List/status | Active owner | Allow |
| AUTH-003 | List/status | ReclaimGrace | Allow |
| AUTH-004 | List/status | Held owner | Allow |
| AUTH-005 | Exclusive attach | Unattached | Allow |
| AUTH-006 | Exclusive attach | Active owner | Reject `already_attached` |
| AUTH-007 | Exclusive attach | ReclaimGrace | Allow only with the matching reclaim token |
| AUTH-008 | Exclusive attach | Held owner | Allow with the current Lease generation |
| AUTH-009 | Input/resize | Unattached | Reject `not_attached` |
| AUTH-010 | Input/resize | Active owner | Allow |
| AUTH-011 | Input/resize | ReclaimGrace | Reject `not_attached` |
| AUTH-012 | Input/resize | Held owner | Reject `not_attached` |
| AUTH-013 | Detach | Unattached | Reject `not_attached` |
| AUTH-014 | Detach | Active owner | Allow; no TTL becomes Unattached, positive TTL becomes Held |
| AUTH-015 | Detach | ReclaimGrace | Reject `not_attached` |
| AUTH-016 | Detach | Held owner | Reject `not_attached` |
| AUTH-017 | Close/task.stop | Unattached | Reject `not_attached` |
| AUTH-018 | Close/task.stop | Active owner | Allow |
| AUTH-019 | Close/task.stop | ReclaimGrace | Reject `not_attached` |
| AUTH-020 | Close/task.stop | Held owner | Allow with the current Lease generation |
| AUTH-021 | Lease claim/takeover | Unattached | Apply normal claim rules |
| AUTH-022 | Lease claim/takeover | Active owner | Reject `already_attached` |
| AUTH-023 | Lease claim/takeover | ReclaimGrace | Reject |
| AUTH-024 | Lease claim/takeover | Held owner | Apply normal claim rules and increment generation |
| AUTH-025 | Lease touch/release | Unattached | Reject |
| AUTH-026 | Lease touch/release | Active owner | Reject |
| AUTH-027 | Lease touch/release | ReclaimGrace | Reject |
| AUTH-028 | Lease touch/release | Held owner | Allow only with matching owner and generation |

The following rows exercise every authority transition. Each positive case
must also assert that old tokens or generations are invalidated.

| ID | Transition or invariant |
|---|---|
| AUTH-029 | Unattached + successful exclusive attach becomes Active |
| AUTH-030 | Active + connection EOF becomes Unattached without a hold |
| AUTH-031 | Active + detach without TTL becomes Unattached |
| AUTH-032 | Active + detach with positive TTL becomes Held with a new generation |
| AUTH-033 | Active + supervisor restart becomes ReclaimGrace |
| AUTH-034 | ReclaimGrace + matching token becomes Active |
| AUTH-035 | ReclaimGrace + deadline becomes Unattached |
| AUTH-036 | Held + attach with generation becomes Active and writes a release tombstone |
| AUTH-037 | Held + touch extends the deadline without changing generation |
| AUTH-038 | Held + expiry or release becomes Unattached |
| AUTH-039 | Held + claim/takeover becomes Held under a strictly greater generation |

## Worker Liveness and Reconciliation

Session/Task `lost` is terminal. Worker `unreachable` is transient health and
does not by itself change Task exit data.

| ID | SQLite state | Runtime observation | Expected result |
|---|---|---|---|
| LIVE-001 | Running Session | Matching healthy worker | Keep Session/Task running |
| LIVE-002 | Running Session | Matching worker process, failed PING | Keep Session/Task alive; health `unreachable` |
| LIVE-003 | Running Session | No worker process; socket stale or absent | Mark Session/Task `lost` |
| LIVE-004 | Running Session | Worker exists with mismatched identity | Mark persisted record `lost`; quarantine runtime entry |
| LIVE-005 | No Session record | Healthy worker | Report runtime entry `orphaned`; reject normal Attach/input |
| LIVE-006 | No Session record | Unreachable worker | Report `orphaned` and `unreachable`; reject normal Attach/input |
| LIVE-007 | Closed Session | Healthy worker with same ID | Report `conflict`; never resurrect |
| LIVE-008 | Any Session | Duplicate workers for one SessionId | Report `conflict`; choose neither |
| LIVE-009 | Task process exited | Healthy worker | Mark Task and Session `exited`; retain final snapshot |
| LIVE-010 | Exited Session | Worker unavailable but matching process | Keep Session `exited`; health `unreachable` |

Detection and startup rows are required in addition to the truth table.

| ID | Case |
|---|---|
| LIVE-011 | Supervisor-owned worker exit is observed through `SIGCHLD`/`waitpid` with P95 at or below 50 ms |
| LIVE-012 | Persistent worker-control EOF marks health `unreachable` immediately and schedules identity verification |
| LIVE-013 | Safety scan runs at least every 2 seconds and uses a 500 ms PING timeout |
| LIVE-014 | Startup reconciliation completes before the supervisor accepts new clients |
| LIVE-015 | One unreachable worker does not block unrelated healthy Sessions |
| LIVE-016 | Reconciliation never silently deletes an orphan or conflicting runtime entry |

## Attach Data Plane

| ID | Case |
|---|---|
| ATTACH-001 | Snapshot registers the subscriber first and live output starts at `next_stream_offset` without gaps or duplicates |
| ATTACH-002 | A second exclusive Attach to the same Session returns `already_attached` |
| ATTACH-003 | `INPUT` carries raw bytes and performs no JSON or base64 transformation |
| ATTACH-004 | `OUTPUT` carries a `u64_be` stream offset followed by raw PTY bytes |
| ATTACH-005 | Resize ordering is observable against the output stream offset |
| ATTACH-006 | Explicit detach either releases authority or creates the requested hold |
| ATTACH-007 | Connection EOF releases Active authority immediately |
| ATTACH-008 | Per-attachment output queue overflow closes that channel with `output_backpressure` |
| ATTACH-009 | `read_only` mode returns `unsupported_mode` in V0 |
| ATTACH-010 | Unknown frame type closes the channel with an explicit protocol error |
| ATTACH-011 | Unknown channel version closes the channel with `incompatible_version` |
| ATTACH-012 | Each attachment has an independent bounded 1 MiB output queue |
| ATTACH-013 | `SNAPSHOT` precedes every `OUTPUT` frame from its recorded next offset |
| ATTACH-014 | Every `OUTPUT` frame is at most 64 KiB |
| ATTACH-015 | CLI detach resets Kitty keyboard, modified-key, bracketed-paste, alternate-screen, cursor, synchronized-output, and mouse modes |

## Core Object Model

| ID | Case |
|---|---|
| OBJ-001 | `session.create` atomically creates one Session, Pane, TaskInstance, worker, and PTY |
| OBJ-002 | Duplicate `session.create` is rejected without duplicate durable records |
| OBJ-003 | A V0 Session always reports exactly one Pane and one TaskInstance |
| OBJ-004 | `task.start` is absent from the public V0 protocol after cutover |
| OBJ-005 | `task.stop` moves TaskInstance to `stopped` and Session to `exited` |
| OBJ-006 | An exited Task retains its final terminal snapshot until Session close |
| OBJ-007 | `session.close` performs ordered teardown and reaches `closed` |
| OBJ-008 | No terminal Session or Task state can transition back to `running` |
| OBJ-009 | A TaskInstance belongs to exactly one Session for its full lifetime |
| OBJ-010 | Lease expiry or release never changes Task or Session lifecycle |

## Durable Idempotency

Idempotency keys are scoped to the logical operation and request payload.
Successful and deterministic error responses are retained for 24 hours.

| ID | Case |
|---|---|
| IDEMP-001 | Repeating a mutating request with the same key and payload returns the original response, including after qwed restart and recovery from a pending `SESSION_CREATE` record, without creating duplicate durable records |
| IDEMP-002 | Reusing a key for a different operation or payload returns `idempotency_conflict` |
| IDEMP-003 | A mutating request without an idempotency key returns `invalid_request` |
| IDEMP-004 | Replay records expire at the retention deadline and expired records are removable |

## History Retention

| ID | Case |
|---|---|
| HIST-001 | A terminal Session is pruned only when older than 30 days and outside the newest 10,000 terminal Sessions; running Sessions and records with attachment authority are retained |
| HIST-002 | Startup maintenance removes eligible terminal Session, Task, and Pane history before qwed accepts clients |

## Stage 1 Hardening Evidence

The following rows now have real-process coverage:

- The missing-worker row: a missing worker process becomes lost.
- The worker-loss latency row: 20 real worker kills are summarized by P95 through
  `just worker-loss-latency`.
- The safety-scan row: an adopted non-child worker is found through
  `just safety-scan-smoke`.
- The unreachable-isolation row: one `SIGSTOP` worker does not block qwed or a
  healthy Session.
- The Pane-cardinality row: `CONTROL_STATUS` returns exactly one persisted Pane for the tested
  Session.
- The creation-path row: `task.start` is absent from the public protocol and CLI.
- The Task-ownership row: foreign-key and unique constraints reject orphan or duplicate
  TaskInstance references.

## Stage 3 Completion Evidence

macOS ARM64 and Fedora Linux x86_64 automated evidence exists for:

- Active Attach persistence, fresh CSPRNG reclaim tokens, and supervisor-restart
  Reclaim Grace.
- Positive-TTL detached holds, wrong owner/generation rejection, Lease touch,
  Hold-to-Active reattach, and release tombstones.
- Lease claim rejection while Active.
- Durable idempotency replay in-process and across qwed restart, conflict
  detection, missing-key rejection, and 24-hour expiry records.
- Unattached, Held, and Active authorization for control input, task stop, and
  Session close, including wrong owner/generation rejection.
- Healthy and unreachable orphans, closed-session conflict, identity mismatch,
  duplicate workers, and non-destructive quarantine reporting.

The AUTH and LIVE policies are unit-tested across every state pair, with
real-process probes covering attach, grace, hold, Lease, close/task-stop,
control input, orphan/conflict, identity mismatch, duplicate workers, and
exited-worker health. The Stage 1-3 audit passes on both required platforms.
