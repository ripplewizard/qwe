# V0 Contract Test Matrices

Status: Stages 1-4 validated on macOS ARM64 and Fedora Linux x86_64; Stage 5
foundation passing on both, with macOS temporarily local-self-signed
Date: 2026-11-06

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

## Stage 4 Services, Security, and V0 Capabilities

These rows define the Stage 4 acceptance surface. A row is complete only when
its named command has evidence on both required platforms.

| ID | Case | Expected result |
|---|---|---|
| SVC-001 | Parse and platform-lint the systemd user unit and launchd agent assets | Required restart, runtime/state-directory, umask, and kill-policy fields are present; malformed manifests fail |
| SVC-002 | SIGKILL qwed while managed by launchd or systemd | The worker and Task survive; the service manager restarts qwed; startup reconciliation adopts the same worker and Session |
| SVC-003 | Stop the managed qwed service with a live Session | qwed handles SIGTERM, terminates worker/Task, persists Session `closed` and Task `stopped`, and exits cleanly |
| SVC-004 | Replace the managed qwed executable before a crash restart | The next service start executes the replacement path, adopts the surviving worker, and preserves the Session |
| SEC-001 | Send malformed JSON in a valid-length client frame | Close that client connection and keep qwed healthy |
| SEC-002 | Send a client frame header above 8 MiB | Reject before reading or allocating the payload and keep qwed healthy |
| SEC-003 | Send control input over 1 MiB | Return `input_too_large` without forwarding input to the worker |
| SEC-004 | Compare matching and mismatched socket peer UIDs | The private runtime socket denies a second UID; a temporary ACL bypass permits connection; qwed then rejects the client with the `unauthorized_peer` policy error |
| CRED-001 | Load a credential from a private regular file and reject missing, insecure, symlink, and oversized sources | Only the private regular file is loaded; failures return `credential_unavailable` |
| CRED-002 | Consume a credential in a real PTY child through the inherited descriptor contract | Secret bytes match exactly and do not appear in argv, environment, protocol status, terminal output, logs, or SQLite |
| CRED-003 | Use an explicitly signed qwed binary against an ACL-bound temporary Keychain | Trusted code signature reads the item; locking the Keychain fails closed without UI; unlocking restores access; public Developer ID distribution remains a release-identity requirement |
| RES-001 | Apply an aggregate Pane CPU-time limit | The whole Session tree is terminated after consumed CPU time crosses the limit |
| RES-002 | Request a resource that the current platform/enforcement tier lacks | Return `unsupported_capability`; never silently downgrade or ignore the request |
| RES-003 | Apply tree memory limits | Linux enforces `memory.max`/`memory.swap.max`; macOS reports monitored enforcement and terminates after sampling a breach |
| RES-004 | Apply a task-count limit | Linux enforces `pids.max`; macOS monitors the process tree; both retain current/peak task metrics |
| RES-005 | Update limits on a running held Session | Worker and durable Pane policy change together; status reports the new effective policy and metrics tier |
| RES-006 | Inspect resource status during enforcement | Current/peak memory and task counts, CPU seconds, tier, and last violation are reported without unbounded history |
| RES-007 | Lower limits below current resource usage | Linux memory triggers cgroup OOM on the Session tree; lower `tasks_max` blocks new forks without killing existing tasks; macOS monitors and terminates on the next breach sample |
| KILL-001 | Force-kill one held Session | Task and Session terminate; process group disappears; authority is released; Session persists as `closed` |
| KILL-002 | Batch-force multiple held Tasks | Every requested Task becomes `stopped` and reports per-item failures without aborting unrelated teardown |
| KILL-003 | Force-kill the server with explicit confirmation | Every Session is torn down, the result is returned, and qwed exits successfully |

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

## Stage 4 Completion Evidence

macOS ARM64 and Fedora Linux x86_64 currently pass `just stage4-check`:

- Private credential loading, descriptor delivery, and secret-redaction checks.
- Aggregate CPU-time enforcement and explicit unsupported-capability responses.
- Linux cgroup v2 hard memory, swap, task, and CPU-bandwidth enforcement.
- macOS libproc current/peak resource monitoring and monitored termination.
- Dynamic Active/Held policy updates persisted in SQLite and applied live.
- Malformed/oversized control frames, oversized input, and peer-UID policy.
- Session, batch Task, and server forced teardown with real processes.
- systemd-unit and launchd-plist structural validation, including macOS
  `plutil` lint.
- Real launchd/systemd crash-restart, worker adoption, graceful stop, and
  terminal-state cleanup.
- Binary path replacement before managed restart, with worker adoption.
- Local signed Keychain ACL, locked-Keychain fail-closed, and unlock
  recovery with no background password dialog.
- Real second-UID socket denial and application peer-UID rejection on macOS
  and Fedora.

All Stage 4 rows are closed. See
[Experiment 0020](experiments/0020-stage-4-closure.md) for the closure record.

Public Developer ID trust and notarized distribution are Stage 5 release
packaging gates. Local signed ACL and locked/unlocked Keychain behavior pass.
Final packaged installation, upgrade migration, and release-mode soak
measurements are also Stage 5 gates.

## Stage 5 Release Candidate

| ID | Case | Expected result |
|---|---|---|
| MAC-001 | Create, use, expose, and remove the temporary local signing identity | Dedicated Keychain signs all four executables; package records `local-self-signed` and `public_trust: false`; public gate rejects it; cleanup removes Keychain/trust and restores the user search list |
| PACK-001 | Build a native release package | All four executables report the manifest version; payload paths are portable; a safely extractable archive, SHA-256 checksums, and `qwe-release/v1` metadata are present |
| PACK-002 | Verify and install a package into an isolated prefix | Checksums pass, executable modes are correct, native service assets are rendered without a checkout path, and no service is started |
| PACK-003 | Reinstall while preserving state, restart qwed, and migrate v9 state | SQLite is preserved during install; the same worker/task are adopted; startup migrates schema v9 to v10 |
| PACK-004 | Sign, notarize, and assess the public macOS artifact | Developer ID signatures are verified, notarization is accepted, and Gatekeeper assessment passes |
| PACK-005 | Exercise the installed Linux service through systemd | The installed unit starts qwed; binary reinstall preserves state; SIGKILL restarts the service; the same worker/task are adopted; stop cleans processes and persists closed/stopped state |
| PERF-001 | Run release-mode Attach and soak | Eight independent Sessions complete real Attach/snapshot/detach cycles within configured latency and aggregate-RSS limits throughout the soak |

Packaged installation, reinstall, worker adoption, schema migration, and
release-mode performance pass on macOS ARM64 and Fedora Linux x86_64. Public
macOS signing/notarization, full manual release acceptance, and final V0 freeze
remain pending.

The local signing lifecycle passes with an isolated create/sign/verify/remove
probe on macOS ARM64. The temporary identity is marked non-public and is not
committed.

The installed-systemd probe passes on Fedora Linux x86_64 with the installed
unit, binary reinstall, supervisor crash-restart, worker/task adoption, and
graceful stop persistence.
