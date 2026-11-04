# ADR 0013: V0 Core Object Model and Lifecycle

Status: Accepted
Date: 2026-11-04
Implementation state: Accepted contract; migration not implemented.

## Context

Workspace, Session, Pane, Task, TaskInstance, and Lease are described in
different documents with different levels of precision. The current code maps
`TASK_START` and `SESSION_CREATE` to the same creation path, and stopping a Task
closes its Session. Those are implicit object-model decisions that were never
accepted.

The public Attach channel cannot be finalized until it is clear what a client
attaches to and whether a Session can own multiple execution instances.

## Decision

For V0, use one explicit simplified graph:

```text
WorkspaceDefinition
  +-- Session 1..N
        |
        +-- Pane 1:1
        +-- TaskInstance 1:1
        +-- PTY and worker 1:1
```

### Object Responsibilities

- `WorkspaceDefinition` is configuration and never owns a runtime lifecycle.
- `Session` is the durable terminal container. In V0 it owns exactly one
  session worker, one PTY, one Pane, and one TaskInstance for its lifetime.
- `Pane` is the terminal presentation and policy target. It references a
  Session and never owns a TaskInstance.
- `Task` is the reusable logical command definition. V0 embeds it in
  `session.create` and does not expose a separate Task-definition lifecycle.
- `TaskInstance` is one concrete execution with a stable TaskId, process
  identity, state, and exit information. V0 APIs expose TaskInstance IDs.
- `Lease` and attachment ownership are control-authority records, not parent
  objects.

Multi-TaskInstance Sessions are deferred to a V1 decision. They must not appear
as an implicit capability in V0.

### Canonical Creation

`session.create` is the only creation operation in V0. It atomically creates the
Session, Pane, sole TaskInstance, worker, and PTY. The response returns Session
and TaskInstance identities.

`task.start` is removed during the pre-release cutover. It must not exist as a
second creation path or as an ambiguous alias. A CLI may provide a convenience
verb only if it calls the same canonical operation.

### Lifecycle

`SessionState`:

```text
creating -> running | lost
running  -> exited | lost | closing
exited   -> lost | closing
closing  -> closed
```

`TaskInstanceState`:

```text
starting -> running | stopped | lost
running  -> exited | stopped | lost
```

No terminal state returns to `running`. An exited Task leaves the worker and
final terminal snapshot available until the Session is closed.

### Teardown Semantics

- `task.stop` signals the Task process group, moves the TaskInstance to
  `stopped`, and moves the Session to `exited`; it does not remove the final
  screen snapshot.
- `session.close` stops any live TaskInstance, removes the worker and sockets,
  releases attachment and Lease authority, and moves the Session to `closed`.
- `kill-session` and `kill-server` are D3 forced-teardown operations and must
  use the same state transitions with an explicit signal policy.

## Invariants

- SessionId, TaskId, PaneId, and WorkspaceId are stable opaque identities.
- A V0 Session has exactly one TaskInstance; it never starts a second one.
- A TaskInstance belongs to exactly one Session.
- A Pane belongs to exactly one Session and does not own execution.
- A Lease never changes Task or Session lifecycle.

## Alternatives Considered

### Support Multiple TaskInstances in V0

Deferred. It requires decisions for output attribution, attach targeting, Pane
layout, and lifecycle aggregation.

### Treat Session and Task as the Same Object

Rejected. The worker can retain an exited Task's final terminal snapshot, and
Lease/attachment lifecycle is independent from process exit.

### Keep Both Creation Paths

Rejected. Two creation operations encode conflicting ownership assumptions and
make idempotency ambiguous.

## Consequences

- Attach can address one Session and one output stream in V0.
- Schema and protocol work may proceed against explicit 1:1 cardinality.
- Existing code must remove the implicit duplicate creation path.
- Multi-TaskInstance support requires a later model ADR.

## Acceptance Required

- Approve V0 Session-to-TaskInstance 1:1 cardinality.
- Approve Pane-to-Session 1:1 cardinality.
- Approve `session.create` as the sole creation operation.
- Approve the Session and TaskInstance state machines and teardown split.
