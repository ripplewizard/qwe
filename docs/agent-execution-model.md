# Agent Execution Model

Status: Stages 9.1-9.5 implemented; accepted Agent Execution first delivery
complete
Date: 2026-11-14
Authority: [ADR 0032](adr/0032-agent-execution-model.md)

## 1. Delivery Boundary

Stages 9.1-9.5 implement the accepted first delivery of the Agent Execution
Model:

- non-PTY `exec` with an explicit program and argument vector;
- a stable command handle and the `starting`, `running`, `done`, `failed`, and
  `lost` states;
- bounded `wait_idle` waiting with `unknown` on timeout;
- separate stdout/stderr events with per-stream cursors;
- a 1 MiB-per-command replay window and explicit replay gaps;
- command metadata persistence and qwed-restart recovery to the owning worker;
- explicit worker-loss handling that marks nonterminal commands `lost`;
- Pane-scope inheritance on Linux;
- CLI commands for execution, status, events, waiting, and termination.
- hashed, expiring capabilities bound to a Principal epoch chain;
- Pane- and command-scoped authorization with no same-UID ambient authority;
- Principal creation, ancestry checks, and real-time revocation;
- administrative, freeze, and terminate severity, including finite freeze
  escalation and visible command events;
- mode-600 local capability files and bootstrap management credentials.
- per-command credential delivery through a one-shot descriptor broker and
  inherited FD3;
- zeroizing secret memory and streaming exact-value redaction before event
  retention, with a visible `redaction_applied` event.
- read-only command observation with bounded push queues and live capability
  rechecks;
- PTY-merged command output with terminal control sequences preserved.
- foreground-process-group idle detection for interactive shell PTY commands.

For an interactive shell PTY command, the worker reads `tcgetpgrp(master_fd)` from the
PTY master. The shell itself becoming the foreground group marks the command
`idle` and commits an `idle_observed` event; a foreground child restores
`running`. Terminal-state checks run first, so `done`, `failed`, and `lost`
always take precedence. Non-PTY fresh processes and shell command-string modes
do not claim idle.

The diagnostic terminal journal is not used for command replay. Agent events and
the terminal journal remain separate formats and retention domains.

## 2. Pane and ResourceScope Ownership

On Linux, qwed now starts the Pane worker inside the transient Pane systemd
scope. The worker creates two delegated child cgroups: `control` hosts the
worker itself, while `workload` hosts the PTY process and agent commands.
Both children remain inside the same Pane scope.

This ordering is required because an ordinary user cannot migrate a process
between arbitrary systemd scopes: cgroup v2 requires write access through the
source and destination common ancestor. Starting the worker inside the Pane
scope and splitting its delegated children avoids a privileged cgroup move,
excludes worker threads from user resource limits, and keeps command accounting
inside the existing Pane ResourceScope.

Worker teardown terminates agent process groups before Pane cleanup. qwed scope
cleanup remains the final backend cleanup authority.

## 3. Current Operations

All requests and responses use the existing versioned control envelope.

| Operation | Mutation | Target |
|---|---|---|
| `AGENT_EXEC` | Yes | Pane |
| `AGENT_COMMAND_STATUS` | No | Agent command |
| `AGENT_COMMAND_EVENTS` | No | Agent command |
| `AGENT_COMMAND_WAIT_IDLE` | No | Agent command |
| `AGENT_COMMAND_TERMINATE` | Yes | Agent command |
| `AGENT_COMMAND_SIGNAL` | Yes | Agent command |
| `AGENT_COMMAND_OBSERVE` | No | Agent command, read-only push |
| `SECURITY_BOOTSTRAP` | No | Local root management capability |
| `SECURITY_PRINCIPAL_CREATE` | Yes | Principal |
| `SECURITY_CAPABILITY_ISSUE` | Yes, non-idempotent | Principal, operation, and scope |
| `SECURITY_PRINCIPAL_REVOKE` | Yes | Principal subtree |

The current request fields and limits are defined in
[IPC Protocol](ipc-protocol.md#agent-command-control). Command IDs are UUIDv7
text values. Request idempotency keys are required for `AGENT_EXEC` and
termination.

## 4. Capability and Principal Model

`AuthContext.capability` is carried only in the request envelope over the
owner-only local socket. Capability issue responses are the only public
operation that returns a raw token; they are intentionally not stored as
idempotent responses. SQLite stores only SHA-256 token hashes.

Capability operations are:

- `manage_principals` with global scope;
- `agent_exec` with Pane scope;
- `agent_command_read` with command scope;
- `agent_command_terminate` with command scope.

An `agent_exec` capability can read, wait for, observe events from, and
signal or terminate only the commands it created. Separate command-scoped read
or terminate capabilities can be issued to other principals; terminate
capabilities may also deliver whitelisted signals.

An `agent_exec` capability may carry one optional `SandboxProfile`. Presence
means every command is executed through the Linux Bubblewrap adapter; absence
means the explicit legacy unsandboxed path. Client `AGENT_EXEC` payloads cannot
choose or replace the profile. See
[ADR 0033](adr/0033-bubblewrap-sandbox-adapter.md).

Every capability records the issuing Principal's complete ancestor epoch chain.
Each request reloads the current chain. A changed epoch, any revoked ancestor,
an expired capability, or a revoked capability fails closed. This makes ancestor
revocation real-time without rewriting descendant capabilities.

The local root Principal is high-trust. qwed creates a 30-day bootstrap
management capability at startup and writes only its raw token to
`<socket>.runtime/security.bootstrap` with mode `0600`. CLI security commands
read bootstrap or an explicit `--capability-file`; agent commands require an
explicit capability file.

Revocation behavior:

- `administrative`: invalidates capabilities but preserves running commands.
- `freeze`: invalidates capabilities and sends `SIGSTOP` to affected command
  groups.
- `terminate`: invalidates capabilities and kills affected command groups.

Missing severity defaults to `freeze`. Freeze grace defaults to 60 seconds and
is capped at 5 minutes. A 250 ms qwed invigilator escalates expired freezes to
termination, including after qwed restart, and the worker records both
`revocation_applied` events. Administrative revocation and security holds
require a high-trust Principal.

## 5. Command Credentials and Redaction

`AGENT_EXEC.credential` names a credential in the existing private provider.
The credential value is never forwarded through the worker JSON request.

qwed reads the private credential into `Zeroizing` memory, writes it to a pipe,
and sends the read descriptor over a one-shot mode-0600 Unix socket using
`SCM_RIGHTS`. The request contains only `credential_socket` and
`redaction_policy=credential_exact`.

The Pane worker receives the descriptor, reads and validates at most 16 KiB into
`Zeroizing` memory, creates a fresh pipe for the child, and maps it to descriptor
3 through the internal launcher. The child sees only `QWE_CREDENTIAL_FD=3`; the
secret never enters argv or an environment value.

The same in-memory value feeds a streaming exact-match redactor for each output
stream. The redactor retains only partial secret prefixes at chunk boundaries,
replaces complete or incomplete trailing matches with `[REDACTED]`, and commits
redacted data to the bounded event log. The first match emits
`redaction_applied`. Empty credentials, descriptor failures, or unsupported
redaction policies fail the command before successful execution is reported.

Credential values are not stored in SQLite, idempotency responses, command
metadata, event data, worker output, or logs.

## 6. Sandboxed Execution

qwed validates and probes the profile when issuing the capability, then injects
the persisted profile into the internal worker request. The worker repeats
profile checks and runs Linux pipe-split or PTY commands:

```text
qwe-worker
  -> qwe-session-exec
       -> bwrap
            -> qwe-sandbox-init (Landlock ABI >= 1)
                 -> command
```

The worker waits for Bubblewrap's structured `child-pid` before reporting
`running`. The status response exposes the enforced profile, outer wrapper PID,
and structured child PID while the command is live. Both PIDs remain in the
Pane workload cgroup. Bubblewrap startup failure, missing namespaces, or an
unsupported profile fails the command or capability request; there is no
sandbox-to-unsandboxed fallback.

When Landlock is available, `qwe-sandbox-init` applies a write-focused ruleset
inside Bubblewrap before executing the command. It permits mutations only under
a read-write workspace, private `/tmp`, and `/dev`; reads, directory listing,
execute, and loader behavior remain available. This is defense in depth, not a
Landlock-only backend. The handled rights grow with the kernel ABI, including
`REFER`, `TRUNCATE`, and device `IOCTL` where supported. The capability audit
records the detected ABI.

For `network:isolated_loopback`, Stage 10 verifies an UP `lo` interface, a real
`127.0.0.1` listen/connect pair, and no default route across Bubblewrap 0.5,
0.6.1, 0.8, and 0.9. Sandbox status reports `setuid_denied:true` and
`no_new_privs:true`; `NoNewPrivs: 1` is verified inside the command.

Sandbox status also reports `risk.network_inherit`,
`risk.writable_workspace`, and `risk.credential_exposed`. Their combinations
identify possible network exfiltration or workspace persistence. A
`sandbox_risk_observed` event is committed when the command is accepted.
Risk flags are persisted in SQLite and survive worker replacement.
Stage 10 confirms both boundaries: an inherited network can send the FD3
credential to a host listener, and a writable workspace can retain the
credential in a host-visible file after the command exits. These are declared
capability risks, not redaction failures.

Sandboxed PTY commands keep the portable-pty controlling terminal and do not use
`--new-session`. A minimal internal seccomp filter rejects `TIOCSTI`; FD3
credentials and redaction use the normal command path. The status response
reports wrapper PID, structured child PID, and `signal_pgid`. Terminal idle and
signals use the current foreground group, falling back to the child process
group.

qwed restart reconnects to the surviving worker and returns the same profile,
wrapper, structured child, command state, and events. Confirmed worker loss
marks nonterminal commands `lost` while retaining the capability profile for
audit. Pane teardown and worker loss both remove Bubblewrap descendants through
the Pane scope and `--die-with-parent`; qwed remains the generic subreaper for
orphaned wrapper processes.

## 7. Observation and PTY

`AGENT_COMMAND_OBSERVE` accepts a command-scoped `agent_command_read` capability
(or the creating `agent_exec` capability), a cursor, and a bounded timeout.
Each iteration rechecks the live Principal epoch chain before polling the
worker. qwed pushes typed `batch` frames over the existing control socket and
ends with `end`; a 32-frame `SyncSender` provides the subscription bound.
A subscriber that does not drain the queue is disconnected rather than forcing
unbounded process buffering.

The observer receives committed metadata and redacted output only. It has no
input, resize, signal, lifecycle, or command-mutation operation. `AGENT_EXEC`
and termination with only a read capability fail with
`capability_scope_denied`.

`AGENT_COMMAND_SIGNAL` accepts `SIGINT`, `SIGTERM`, `SIGHUP`, and `SIGQUIT`.
PTY signals target the current foreground process group; non-PTY signals target
the command process group. Each delivery appends a `signal_delivered` event.

`AGENT_EXEC` with `pty:true` uses a fresh PTY and one merged `pty` event stream.
The terminal control bytes are retained, CR/LF translation follows normal PTY
semantics, and the same credential/redaction path applies. Non-PTY commands keep
separate stdout/stderr streams. PTY and pipe commands share command identity,
cursor rules, lifecycle, replay bounds, and termination semantics.

## 8. Event and Retention Contract

Each event has a command-global monotonic sequence, timestamp, and typed kind.
Output events also have a stream and byte cursor. Ordering is per stream; the
global sequence is metadata, not a claim that stdout and stderr were observed
in a total wall-clock order.

The worker retains at most approximately 1 MiB of semantic output and event
metadata per command. Older events are evicted without renumbering. A read from
an evicted cursor returns `replay_gap` with the unavailable range.

Event retention is in-memory and ends when the worker is replaced. qwed persists
command identity, state, process identity, policy-independent metadata, exit
result, and timestamps, but not event output.

## 9. Recovery Truth Table

| Observation after qwed reconnect | Result |
|---|---|
| Same worker and command are reachable | Return current worker state and events |
| Worker command exists but is terminal | Return recorded exit result |
| Worker no longer owns the command | Persist and return `lost` |
| Pane worker is confirmed absent | Persist all nonterminal commands as `lost` |
| Worker is temporarily unreachable but not confirmed lost | Return a transport error; do not guess `lost` |

Recovery never converts missing process or output state into success.

## 10. Deferred Follow-ons

| Follow-on | Required work |
|---|---|
| `shell_exec` | Explicit shell selection, quoting, login/interactive defaults, and separate tests |
| Cross-Pane orchestration | Separate orchestration design and authority model |

The capability layer prevents ambient authority between principals even when
clients share one UID. It does not replace OS multi-user isolation:
same-UID processes can still read the root bootstrap file. Environment mutation,
`shell_exec`, Session/Pane startup-program sandboxing, cross-Pane
orchestration, persistent history, and host management remain outside this
delivery.

`shell_exec`, broader sandbox backends and deployment qualification, cross-Pane
orchestration, persistent semantic history, host management, and multi-writer
commands remain deferred.

## 11. Validation

Run:

```sh
just check-qwe
just stage9-agent-exec
just stage9-check
just stage8-layout
just stage7-resource-profile
just stage7-multi-pane
```

Stages 9.1-9.5 are represented by AGENT-001 through AGENT-012 in the
[test matrices](test-matrices.md#agent-execution). The real-process probe
covers exit codes, signals, start failure, deadlines, wait timeout, split
streams, bounded replay, Pane-cgroup inheritance, CLI use, qwed restart, and
worker-loss `lost` reporting, as well as missing/cross-Pane/expired
capabilities, ancestor revocation, administrative preservation, default freeze,
freeze escalation, low-trust administrative denial, and token non-persistence.
It also verifies credential descriptor delivery, FD3, split-chunk stdout/stderr
redaction, no secret in argv/environment/SQLite/logs, and failure on missing or
empty credentials.
Observer evidence covers multi-frame push, read-only denial, and CLI output;
PTY evidence covers merged `pty` events, terminal control bytes, CR/LF
translation, descriptor credentials, and split-chunk redaction.
Idle evidence verifies interactive-shell foreground-group detection,
`idle_observed`, command termination from idle, and non-idle one-shot PTY
completion.
