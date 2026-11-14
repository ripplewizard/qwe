# ADR 0032: Agent Execution Model

Status: Accepted
Date: 2026-11-14
Extends: [ADR 0021](0021-shared-prerequisites.md)
Implementation state: Design accepted; implementation has not started.

## Context

ADR 0021 establishes one authoritative kernel, one worker per Pane, Pane-first
resource scopes, `ChannelProfile`, an opaque `AuthContext`, and the minimum
Principal tombstone model. It deliberately leaves command execution semantics,
agent event delivery, capability transport, revocation severity, and shell
integration open.

QWE must not implement agent commands by injecting shell strings into an
existing Pane Attach channel. That would combine quoting, command boundaries,
timeouts, stdin, output attribution, and credential handling into one
ambiguous interface. Agent execution therefore needs an explicit command
object and channel contract while reusing the existing Pane, Task,
ResourceScope, credential, and authority model.

This ADR resolves the implementation-plan blockers identified in
[Feature Assessment](../feature-assessment-agent-runtime-and-ux.md):

- `exec` semantics are selected and `shell_exec` is deferred;
- command handles and `wait_idle` are one indivisible delivery;
- capability transport and revocation severity are selected;
- nested tenants use real-time ancestor epoch checks;
- bounded scrollback copy is already supplied by Stage 8 and is not part of
  the agent execution channel.

## Decision

### 1. One execution state path

Agent execution remains a channel profile over the existing Session, Pane,
Task, worker, resource, authority, and persistence model. It must not create a
second PTY owner, Session state machine, resource model, or attachment
authority path.

An agent command is hosted by the Pane worker named by `agent_exec`. It runs in
that Pane's process tree and ResourceScope. A later Task-scoped enforcement
backend may refine resource attribution, but the absence of a Task scope cannot
cause execution to escape the Pane scope.

### 2. `exec` is the first command primitive

The first implementation provides `exec` only.

`exec` starts a fresh child process by resolving one program path and an
argument vector. It does not interpret a shell command string. Pipes,
redirection, glob expansion, aliases, and shell syntax are available only when
the caller explicitly starts a shell as `program`.

The initial request includes:

- `program`;
- `args[]`;
- optional `cwd`;
- optional terminal rows and columns when the caller requests a PTY;
- optional deadline;
- an idempotency key;
- an opaque `AuthContext`.

Environment mutation and credential references use explicit fields. Secret
values never appear in argv, environment, logs, SQLite text, or agent context.

`shell_exec` is deferred. If added later, it must be a separate operation with
explicit shell selection, non-login/non-interactive defaults, quoting
semantics, and its own tests. It cannot silently become the implementation of
`exec`.

### 3. Command handle and lifecycle

Each successful start returns a stable `AgentCommandId` and a command handle.
The handle identifies:

- Session and Pane identity;
- the worker identity that owns the process;
- command identity and process identity;
- start state and exit state;
- output cursor;
- effective resource policy and scope identity;
- the capability and Principal that created it.

Command states are:

| State | Meaning |
|---|---|
| `starting` | Accepted and spawning; no reliable terminal state yet |
| `running` | Process is alive and a foreground/child process is active |
| `idle` | Process is alive but no active foreground process is running |
| `done` | Process exited normally; exit code is retained |
| `failed` | Process failed to start or exited because of a signal |
| `lost` | Ownership cannot be proved after worker or supervisor recovery |

`idle` is not terminal. `done`, `failed`, and `lost` are terminal. A recovered
command whose process identity cannot be proved is `lost`; it is never
optimistically reported as `done`.

`wait_idle` and the command handle are delivered together. `wait_idle` returns
`idle`, `done`, `failed`, `lost`, or `unknown`, with a bounded wait. A timeout
returns `unknown`, not `idle`. `done` always takes precedence over `idle`.

Foreground-process-group detection is extracted as one internal primitive
shared by `wait_idle`, blocked-input detection, and shell signal delivery.

### 4. Output and channels

`agent_exec` starts and controls commands. `agent_stream` and
`agent_observer` target an `AgentCommandId`, not an entire Session.

Output modes are explicit:

- A PTY command uses one merged output stream and retains terminal control
  semantics.
- A non-PTY `exec` exposes separate stdout and stderr streams, each carrying a
  monotonically increasing event sequence and a shared command cursor.
- Ordering is guaranteed within a stream. Cross-stream timing is not presented
  as a total order beyond the recorded sequence metadata.

The first delivery may implement one mode, but the contract must represent
both without changing command identity or the event schema.

Each subscriber has a bounded queue. Slow subscribers receive a backpressure
failure or reconnect from a retained bounded replay window; they cannot force
unbounded process buffering. Output frames remain bounded to the existing
64 KiB frame limit, and retained replay is bounded to 1 MiB per command unless
a later ADR changes the limit.

An observer receives committed metadata and output events only. It cannot
write stdin, resize a PTY, signal a process, release a Lease, or mutate a
command.

### 5. Events and history

Agent history is a bounded semantic event log, separate from the diagnostic
worker journal.

The initial event types are:

- command accepted;
- process started;
- output committed with stream and cursor;
- state changed;
- idle observed;
- terminal result with exit code or signal;
- command lost;
- redaction applied;
- retention or replay gap.

The log records command identity, timestamps, state transitions, cursors,
exit status, resource summary, and redaction outcomes. It does not retain a
searchable terminal recording, semantic command history, or complete
scrollback.

Replay is cursor-based and bounded. A replay gap is explicit and does not
silently renumber events.

### 6. Capability and Principal model

Every profile verifies a capability against the current Principal state.
Capabilities are scoped to an operation and one or more of:

- Session;
- Pane;
- Agent command;
- event subscription.

No capability is ambient across a Session or Pane merely because the client
has the same UID.

The capability token is delivered through the authenticated local socket as
part of the opaque `AuthContext`. It is not passed through argv or environment.
A command may receive an inherited descriptor containing a scoped credential,
but the descriptor contract is separate from the authorization token.

Nested tenants use real-time ancestor epoch checks. Revoking a Principal
invalidates every descendant capability and command authorization without
walking or rewriting a token graph.

### 7. Revocation severity

Revocation has explicit severity:

- Administrative revocation blocks new capability use but preserves running
  work by default.
- Security suspension freezes running work and prevents writes, new commands,
  new credentials, and new observers.
- Termination explicitly kills the affected process trees and closes command
  handles.

Unspecified severity fails safe to freeze. Only a high-trust Principal may
select administrative revocation. A freeze has a finite grace period; the
default is 60 seconds and the maximum is 5 minutes. Expiry escalates to
termination and produces a visible event. A security suspension cannot remain
effective indefinitely without an active high-trust hold.

### 8. Credentials

Per-command credentials are delivered to the spawned process through inherited
descriptors or another non-serialized handle. They are never placed in argv,
the environment, SQLite text, event metadata, observer projections, or
terminal output.

The agent event layer applies explicit redaction before persistence and before
observer delivery. Failure to apply a required redaction policy fails closed.

### 9. Recovery

On worker or supervisor restart, qwed reconciles the command against persisted
identity and the worker-owned process tree:

- a proved live process remains running or idle;
- a proved terminal process becomes done or failed with its recorded exit
  result;
- an absent process whose start token can be proved becomes done only when a
  terminal record exists;
- otherwise the command becomes lost.

Recovery never guesses that missing output or process state means success.

### 10. First delivery boundary

The first implementation includes:

- `exec` with explicit program and arguments;
- command handle and `wait_idle`;
- bounded events, replay cursor, and backpressure;
- read-only command observer;
- capability enforcement and revocation;
- per-command credentials and redaction;
- recovery with explicit lost-state behavior.

It does not include:

- `shell_exec`;
- a core sandbox implementation;
- cross-Pane orchestration;
- persistent terminal or semantic command history;
- privileged host management;
- gRPC, HTTP, SSE, or TCP listeners;
- core-dump policy;
- multiple writers to one command or Pane.

## Validation

Implementation must add stable test-matrix rows before being named after this
ADR. Required coverage includes:

- start, exit code, signal, idle, done, unknown, and lost;
- explicit program semantics with a supported shell and fallback shell;
- PTY merged output and non-PTY split streams;
- cursor replay, replay gaps, bounded queues, and slow subscribers;
- observer cannot input, resize, signal, or control;
- capability scope failures and cross-Pane denial;
- real-time ancestor epoch invalidation;
- administrative, freeze, and terminate revocation severity;
- unspecified severity defaults to freeze;
- freeze timeout escalation;
- credential redaction in output, events, logs, and SQLite;
- worker and qwed restart recovery;
- complete process and ResourceScope cleanup.

## Consequences

- Agent execution gains a stable command identity without a second runtime.
- Every command uses the existing Pane worker and resource boundary.
- `exec` is safer and more deterministic than a shell-string RPC.
- Shell convenience is postponed until its quoting and lifecycle semantics are
  explicit.
- Nested-tenant security adds real-time epoch checks on command and channel
  operations.
- Bounded event replay and explicit lost states prevent false success
  reporting after recovery.

## References

- [ADR 0021](0021-shared-prerequisites.md)
- [Feature Assessment: Agent Runtime and UX](../feature-assessment-agent-runtime-and-ux.md)
- [Technical Architecture](../technical-architecture.md)
- [IPC Protocol](../ipc-protocol.md)
