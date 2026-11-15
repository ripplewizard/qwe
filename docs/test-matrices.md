# V0 Contract Test Matrices

Status: Stages 1-8 complete for the current baseline; installed-systemd matrix
and extended Linux long-run qualification pass; Agent Execution ADR accepted
Date: 2026-11-14

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
| ATTACH-009 | `read_only` opens a qwed-owned observer that receives snapshot/output but cannot input, resize, detach, or control |
| ATTACH-010 | Unknown frame type closes the channel with an explicit protocol error |
| ATTACH-011 | Unknown channel version closes the channel with `incompatible_version` |
| ATTACH-012 | Each attachment has an independent bounded 1 MiB output queue |
| ATTACH-013 | `SNAPSHOT` precedes every `OUTPUT` frame from its recorded next offset |
| ATTACH-014 | Every `OUTPUT` frame is at most 64 KiB |
| ATTACH-015 | CLI detach resets Kitty keyboard, modified-key, bracketed-paste, alternate-screen, cursor, synchronized-output, and mouse modes |

## Session Layout

These rows cover the first Stage 8 compositor slice. They require real Pane
workers and Pane-targeted Attach channels.

| ID | Case |
|---|---|
| LAYOUT-001 | `SESSION_LAYOUT` deterministically tiles two Panes into non-overlapping rectangles that cover the requested viewport |
| LAYOUT-002 | `ATTACH_OPEN` with a `pane_id` selects that Pane and the accepted/snapshot sequence identifies the same Pane |
| LAYOUT-003 | One client can hold distinct exclusive Attach channels to two Panes in the same Session |
| LAYOUT-004 | Raw `INPUT` on a Pane-targeted Attach reaches only the selected Pane |
| LAYOUT-005 | The interactive compositor renders both Panes, switches focus without leaking input, and detaches cleanly |
| LAYOUT-006 | Focus and Pane layout order survive qwed restart without changing Pane identity |
| LAYOUT-007 | The compositor splits after the focused Pane, attaches the new worker, and closes that Pane without disturbing survivors |
| LAYOUT-008 | Reordering through the compositor persists the new layout order and keeps input on the focused Pane |
| LAYOUT-009 | Copy mode selects only the focused Pane's visible screen and reports clipboard provider success or failure explicitly |
| LAYOUT-010 | Quick-menu keys are intercepted by the client and never reach the PTY |
| LAYOUT-011 | A floating Pane overlays tiled Panes, receives focused input, and closes with its worker and ResourceScope |
| LAYOUT-012 | Floating move/resize saves explicit bounds and `SESSION_LAYOUT` returns the persisted rectangle |
| LAYOUT-013 | Simulated SGR mouse press/motion/release on a floating title row persists the dragged rectangle |
| LAYOUT-014 | Dragging the floating bottom-right resize handle persists the resized rectangle |
| LAYOUT-015 | Clicking an exposed lower floating Pane raises it above overlapping overlays and persists z-order |
| LAYOUT-016 | Copy mode scrolls client VT scrollback and copies selected history while excluding newer off-screen lines |
| LAYOUT-017 | Mouse press/motion/release in copy mode selects only focused-Pane text and writes the clipboard |

## Agent Execution

The accepted Agent Execution first delivery covers all rows below.

| ID | Case |
|---|---|
| AGENT-001 | Start explicit non-PTY `exec` and verify a stable command handle reaches `done` with the exact exit code |
| AGENT-002 | Retain output beyond the replay window and verify bounded retention, stable sequence numbers, and an explicit gap |
| AGENT-003 | Run `exec` from a hard-limited Pane and verify the worker control cgroup and shared PTY/command workload cgroup remain under one Pane scope |
| AGENT-004 | Restart qwed, re-adopt live commands, then prove confirmed worker loss produces `lost` rather than false success |
| AGENT-005 | Emit stdout and stderr separately and recover exact bytes from monotonic per-stream cursors |
| AGENT-006 | Verify wait timeout returns `unknown` and a signaled command returns `failed` with signal identity |
| AGENT-007 | Deny missing, cross-Pane, expired, and ancestor-revoked command capabilities in real time |
| AGENT-008 | Cover administrative, freeze, and terminate revocation severity, including finite freeze escalation |
| AGENT-009 | Inject a command credential and prove output, events, logs, and SQLite are redacted or fail closed |
| AGENT-010 | Verify a read-only observer receives committed events but cannot input, resize, signal, or mutate |
| AGENT-011 | Run a PTY command and verify one merged stream preserves terminal control semantics |
| AGENT-012 | Verify foreground `idle` is distinct from task exit and that `done` takes precedence |
| AGENT-013 | Deliver whitelisted signals to PTY foreground/non-PTY process groups, record delivery, and reject SIGKILL and arbitrary signal numbers |

## Bubblewrap Sandbox

Rows 001-034 pass for the Linux `AGENT_EXEC` slice, including sandboxed PTY,
foreground idle, `TIOCSTI` denial, restart, worker loss, and teardown. Row 035
passes the local real-PTY/PID-namespace shim contract; it is not yet part of
production sandboxed Pane or the remote distribution matrix. Ubuntu 22.04 and
Debian 12 real VMs pass the complete `AGENT_EXEC` matrix. Default Ubuntu
24.04 returns `sandbox_backend_unavailable`; with the packaged path-scoped
AppArmor profile and the global userns restriction still enabled, its complete
matrix passes.
Loopback UP/connect and `NoNewPrivs: 1` pass on Bubblewrap 0.5, 0.6.1, 0.8,
and 0.9.
Evidence: [Experiment 0049](experiments/0049-stage10-sandbox-integration.md).

| ID | Case |
|---|---|
| SANDBOX-001 | Preserve FD3 and Pane workload cgroup while reading structured Bubblewrap child PID |
| SANDBOX-002 | Compare Bubblewrap PTY modes and verify resize, Ctrl-C, Ctrl-Z, job control, foreground mapping, and idle |
| SANDBOX-003 | Verify synchronized `--die-with-parent` cleanup after the command creator thread exits |
| SANDBOX-004 | Fail closed when Bubblewrap or unprivileged user namespaces are unavailable |
| SANDBOX-005 | Authorize only capability-declared sandbox profiles and reject unsandboxed downgrade |
| SANDBOX-006 | Enforce workspace protected paths, hidden home, private tmp, and network policy |
| SANDBOX-007 | Recover or terminate sandboxed commands across qwed/worker restart and Pane teardown |
| SANDBOX-008 | Apply the content-hashed `strict` named seccomp denylist and adapter `TIOCSTI` denial in pipe-split and PTY modes, reject unknown profile names, and persist the filter hash in capability audit metadata |
| SANDBOX-009 | Reject a Bubblewrap backend whose help output lacks a production-required option before issuing a capability token |
| SANDBOX-010 | Confirm that a writable workspace plus FD3 credential permits credential persistence, and that the command exposes and persists the corresponding risk flags |
| SANDBOX-011 | Apply optional Landlock write confinement inside mandatory Bubblewrap confinement, record the detected ABI, and preserve reads, execution, and private `/tmp` |
| SANDBOX-012 | Fail capability issue closed with `sandbox_init_unavailable` when Landlock is available but the init helper is missing or non-executable |
| SANDBOX-013 | On Ubuntu 24.04, load the path-scoped Bubblewrap AppArmor profile while preserving the global userns restriction and pass the complete sandbox matrix |
| SANDBOX-014 | On Landlock ABI 4, enforce cross-workspace `REFER` and outside-workspace `TRUNCATE` while preserving workspace, private tmp, and read behavior |
| SANDBOX-015 | With user namespaces globally disabled on Ubuntu 22.04, Ubuntu 24.04, and Debian 12, reject sandbox capability issue while an ordinary Pane command still completes successfully |
| SANDBOX-016 | Run the complete Stage 10 probe while SELinux remains Enforcing on Fedora 44 |
| SANDBOX-017 | Build Bubblewrap 0.5.0 from the pinned source archive and pass the complete Stage 10 probe on Ubuntu 22.04, Ubuntu 24.04 with AppArmor, and Debian 12 |
| SANDBOX-018 | Reject a protected-path profile containing hardlinked files so a writable alias cannot bypass read-only protection |
| SANDBOX-019 | Verify abstract Unix sockets are reachable with `network:inherit` and isolated with `network:isolated_loopback` |
| SANDBOX-020 | On Landlock ABI 5+, permit device ioctls under `/dev` and reject device ioctls outside allowed write roots |
| SANDBOX-021 | Canonicalize a symlinked protected path before hardlink inspection and reject hardlinks reachable through the target directory |
| SANDBOX-022 | Resolve protected paths and workspace cwd with `openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)` and reject absolute symlink escapes |
| SANDBOX-023 | Verify sandbox commands inherit only standard descriptors plus the explicit credential FD |
| SANDBOX-024 | Confirm that a writable workspace FIFO is an intentional host IPC channel for an exposed FD3 credential |
| SANDBOX-025 | Apply `landlock: off | best_effort | required{min_abi}` profile semantics, audit effective ABI, and fail required modes when the ABI is insufficient |
| SANDBOX-026 | Compare nested Bubblewrap under strict seccomp with and without Landlock and prove shim stacking is the monotonic refinement path |
| SANDBOX-027 | Verify PTY-master resize delivers SIGWINCH while foreground process group identity remains stable |
| SANDBOX-028 | Verify PID-1 default SIGTERM behavior, explicit SIGTERM/SIGINT delivery, and SIGCHLD reaping |
| SANDBOX-029 | Verify seccomp socket-creation denial while an inherited Unix socket remains usable |
| SANDBOX-030 | Record and verify Landlock ABI/accepted-rights behavior across target distributions |
| SANDBOX-031 | Verify the strict seccomp denylist under Python, shell, Rust, threads, subprocess, SSL, and nested Bubblewrap runtimes |
| SANDBOX-032 | Detect `--bind-fd`/`--ro-bind-fd` with help plus a harmless FD mount, report the backend feature, and persist it in sandbox audit metadata |
| SANDBOX-033 | Construct refined profiles only when fixed fields are equal and workspace/network/seccomp/Landlock fields do not broaden |
| SANDBOX-034 | Accept the `sandboxed_pane`, `pane_exec`, `pane_input`, and `pane_resize` capability operations only with their intended scopes; require a high-trust issuer for `pane_input` |
| SANDBOX-035 | Run the PID-1 shim under a real PTY, inherit the controlling terminal, forward TERM/INT/HUP, deliver resize SIGWINCH, reap children, and terminate background namespace processes after shell exit |

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

Public Developer ID trust and notarized distribution move to the post-V0 macOS
milestone under ADR 0020. Local signed ACL and locked/unlocked Keychain behavior
pass. Linux packaged installation, upgrade migration, and release-mode soak
measurements pass.

## Stage 5 Release Candidate

| ID | Case | Expected result |
|---|---|---|
| MAC-001 | Create, use, expose, and remove the temporary local signing identity | Dedicated Keychain signs all four executables; package records `local-self-signed` and `public_trust: false`; public gate rejects it; cleanup removes Keychain/trust and restores the user search list |
| PACK-001 | Build a native release package | All four executables report the manifest version; payload paths are portable; a safely extractable archive, SHA-256 checksums, and `qwe-release/v1` metadata are present |
| PACK-002 | Verify and install a package into an isolated prefix | Checksums pass, executable modes are correct, native service assets are rendered without a checkout path, and no service is started |
| PACK-003 | Reinstall while preserving state, restart qwed, and migrate v9 state | SQLite is preserved during install; the same worker/task are adopted; startup migrates schema v9 through the current schema v17 |
| PACK-004 | Sign, notarize, and assess the public macOS artifact | Developer ID signatures are verified, notarization is accepted, and Gatekeeper assessment passes |
| PACK-005 | Exercise the installed Linux service through systemd | The installed unit starts qwed; binary reinstall preserves state; SIGKILL restarts the service; the same worker/task are adopted; stop cleans processes and persists closed/stopped state |
| PERF-001 | Run release-mode Attach and soak | Eight independent Sessions complete real Attach/snapshot/detach cycles within configured latency and aggregate-RSS limits throughout the soak |

Packaged installation, reinstall, worker adoption, schema migration, release
acceptance, and release-mode performance pass for Linux-first V0. Fedora
Linux x86_64 is the supported V0 target. Public macOS signing/notarization is
post-V0 work and is not claimed.

The local signing lifecycle passes with an isolated create/sign/verify/remove
probe on macOS ARM64. The temporary identity is marked non-public and is not
committed.

The installed-systemd probe passes on Fedora Linux x86_64 with the installed
unit, binary reinstall, supervisor crash-restart, worker/task adoption, and
graceful stop persistence.

## Stage 6 Adoption and Stabilization

| ID | Case | Expected result |
|---|---|---|
| STAGE6-001 | Run hosted core CI | Formatting, Clippy, workspace tests, protocol validation, and release builds pass on Linux and macOS without creating a new platform-support claim |
| STAGE6-002 | Run the release gate on the dedicated Fedora runner | The complete `just release-gate` passes on Fedora x86_64 with a live systemd user manager and uploads the archive plus SHA-256 |
| STAGE6-003 | Publish a clean-source release | The release manifest names the tagged source commit with `source_dirty: false`; the archive hash, installation, and service recovery match the published evidence |
| STAGE6-004 | Close the Linux pilot | Reports are triaged, no P0/P1 defect remains open, and at least one `v0.1.x` patch release path is exercised |

STAGE6-001 passes at `66e05a8`. A clean Fedora control run also passed, but it
did not execute through the dedicated repository runner, so STAGE6-002 remains
open. The hosted run, control-run results, manifest, and archive checksum are
recorded in
[Experiment 0026](experiments/0026-stage6-adoption-baseline.md). STAGE6-002
through STAGE6-004 close only when their linked runner, publication, and pilot
evidence exists.

## Linux Container Compatibility

| ID | Case | Expected result |
|---|---|---|
| CONTAINER-001 | Run QWE Session operations in glibc-based Linux containers | Session creation, input, shell function output, snapshot, Task history, supervisor restart adoption, and clean server teardown pass |

CONTAINER-001 passes for Ubuntu 22.04, Ubuntu 24.04, and Debian 12 in
[Experiment 0027](experiments/0027-container-compatibility.md). This is
compatibility smoke only and does not promote those distributions to supported
release targets.

## Stage 6.4 Spike and Stage 6.5 Decision

| ID | Case | Expected result |
|---|---|---|
| SHARED-001 | Validate the ADR 0021 shared prerequisites in a disposable spike | Two to four Pane workers run under one logical Session with isolated terminal state, per-Pane cgroup scopes, Attach targeting, a read-only observer, migration preservation, recovery, health, and complete teardown |
| SHARED-002 | Integrate multiple Pane workers through production qwed | qwed inventories workers by Pane and owns discovery, adoption, health, cleanup, Attach routing, and observer channel persistence |
| SHARED-003 | Migrate a populated v10 database through schema v12 | Sessions, Panes, Tasks, Leases, attachment authority, idempotency records, policy, layout metadata, and history survive with stable identities |
| SHARED-004 | Recover multiple Pane workers through production qwed | qwed discovers, adopts, reports health for, and routes operations by Pane; one Pane loss does not mark unrelated Panes lost |
| SHARED-005 | Exercise interactive and observer channels on one Pane | Interactive authority is exclusive for that Pane; the qwed-owned observer receives snapshot/output only and cannot input, resize, or control |
| SHARED-006 | Tear down a Pane ResourceScope with detached jobs | The complete Pane scope closes without residual processes or controller state, and the cleanup tombstone records success |

The shared-prerequisites spike passes for two- and four-Pane runs on Fedora Linux x86_64 with
systemd user cgroup v2 delegation in
[Experiment 0028](experiments/0028-stage6-shared-prerequisites-spike.md). The
result uses a spike-local supervisor and direct worker Attach. Production
qwed multi-Pane inventory, recovery, interactive channels, and qwed-owned
observer channels now pass through `just stage7-multi-pane`.
Experiment 0028 recommended the accepted observer projection and
complete-scope-teardown rules.

Stage 6.5 accepted ADR 0021 on 2026-11-09 with Pane-first enforcement,
Pane-keyed inventory and Attach, the qwed-owned observer projection, complete
scope teardown, and the v11 migration invariants in
[Storage Migration](storage-migration.md). The production shared-prerequisite
rows are implemented under Stage 7. Schema v12 adds the durable Pane layout
metadata governed by ADR 0023.

## Stage 7 Resource Governance

| ID | Case | Expected result |
|---|---|---|
| RG-001 | Apply Pane resource profiles | Requested and effective policy are visible; explicit field overrides and invalid combinations are deterministic |
| RG-002 | Detect Linux cgroup capabilities | Missing systemd user manager or controllers return `unsupported_capability`; no limit is silently ignored |
| RG-003 | Exercise Pane scope lifecycle failures | Normal exit, OOM, forced kill, detached jobs, worker crash, and qwed restart leave no residual Pane scope |
| RG-004 | Use a Pane under sustained pressure | Memory, CPU, and task limit violations affect that Pane while qwed remains responsive |
| RG-005 | Qualify installed-systemd Linux targets | Packaged service, cgroup delegation, limits, recovery, and cleanup pass before support is claimed |
| RG-006 | Retain long-run resource events | Enforcement and pressure history remains bounded with an explicit retention policy |

The Stage 7 deterministic probe covers profile resolution, controller
rejection, requested/effective policy, crash-restart persistence, bounded
events, short-lived startup acknowledgement, detached-job scope cleanup, and
concurrent memory/CPU/task pressure with qwed latency. The short soak covers
eight Sessions with two Panes, qwed restarts, churn, Attach cycles, and scope
cleanup. Fedora and the Ubuntu 22.04, Ubuntu 24.04, and Debian 12 real-VM
targets pass the installed-systemd probes. The five-hour operation-dense
release soak remains open. History-scale regressions cover startup and
teardown with thousands of completed Panes; see
[Experiment 0041](experiments/0041-linux-vm-qualification.md) for the restart
reconciliation defect and its regression evidence.
