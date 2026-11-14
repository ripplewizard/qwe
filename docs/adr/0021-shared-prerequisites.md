# ADR 0021: Shared Prerequisites for Session Layout and Agent Execution

Status: Accepted
Date: 2026-11-09
Decision gate: Stage 6.5 accepted on 2026-11-09.
Implementation state: Schema v11, Pane-keyed inventory/recovery, multi-Pane
control routing, the qwed-owned observer projection, and complete Pane scope
teardown are implemented.

## Context

QWE currently has one worker per Session, one Pane per Session, one PTY, one
VT parser, one journal, and a per-Session Linux cgroup scope. That model was
sufficient for V0, but three future tracks share assumptions that the current
model does not yet express:

- resource governance needs attribution for Sessions, Panes, and command
  trees;
- Session layout and floating programs need more than one terminal surface;
- agent execution needs a structured channel without creating a second
  Session/Pane state path.

Leaving these assumptions implicit would force Stage 7 resource work, Stage 8
layout work, and E05 agent work to invent incompatible versions of the same
concepts. This ADR defines only the shared prerequisites. It explicitly does
not decide command-execution semantics, the complete capability-token design,
sandboxing, or the compositor.

This ADR amends the one-Pane-per-Session cardinality in
[ADR 0013](0013-core-object-model.md) and supersedes the one-worker/one-PTY
part of Design Baseline D053. V0 behavior remains unchanged until schema v11
and the multi-Pane protocol are implemented.

## Decision

### 1. One kernel, multiple channel profiles

QWE remains one authoritative kernel:

```text
qwed authority
  |
  +-- human channel profiles
  +-- agent channel profiles
  |
  +-- Session -> Pane workers -> PTY / VT / process tree
  +-- Lease / resource scope / event log
```

Human and agent clients are channel profiles over the same Session, Pane,
Task, PTY, and resource state. They must not maintain separate PTYs or
separate authoritative object graphs.

### 2. ResourceScope and ResourceBackend

Introduce a `ResourceScope` abstraction independent of cgroup details.

A scope has:

- a stable opaque `ResourceScopeId`;
- a target (`Session`, `Pane`, or `Task`/command);
- an optional parent scope;
- a backend (`linux_cgroup_v2` or `macos_monitored`);
- requested and effective policy;
- current/peak usage and enforcement events;
- lifecycle state and a cleanup/tombstone record.

The logical hierarchy is:

```text
Session scope
  +-- Pane scope
        +-- Task/command scope
```

`ResourcePolicy` remains the policy value object. `ResourceBackend` owns
backend-specific operations: create/apply/read/close/recover. cgroup paths,
systemd unit names, and `/proc` details never become public protocol fields.

On Linux, the backend must use cgroup v2 and report `unsupported_capability`
when a requested controller or delegation is unavailable. If systemd scopes
cannot express the required parent/child hierarchy, the backend may use a
delegated cgroup subtree, but it must not silently approximate a hard limit.
On macOS, the backend reports monitored semantics and never claims hard
cgroup-equivalent enforcement.

The first implementation target on Linux is the Pane scope. A Session scope is
the logical parent, and a Task scope remains representable but is not enforced
until command execution semantics are accepted. Pane-level enforcement means
that one worker per Pane owns exactly one hard resource boundary in the first
Stage 7 delivery.

The interface must already allow Session, Pane, and Task targets so that later
layout, floating programs, and agent commands do not require a second resource
model. Scope lifecycle states are `creating`, `active`, `closing`, `closed`,
and `lost`. A closed or lost scope retains a cleanup tombstone until its
backend confirms that no owned processes or controller state remain.

### 3. Worker topology: one worker per Pane

Select **one worker per Pane**. A Session is a logical grouping of one or more
Pane workers.

Each Pane worker owns:

- its Pane identity and Pane-local state;
- one PTY and process group;
- one VT parser and bounded scrollback;
- one journal;
- one Pane `ResourceScope`.

qwed remains the authority for Session, Pane, Task, Lease, and worker
inventory. It addresses worker operations, persisted worker identity, runtime
socket discovery, crash recovery, and health by Pane, not by an implicit
"the Session has exactly one worker" assumption.

Non-PTY `exec` children may be hosted by a Pane worker within that Pane's
resource scope. The exact `exec`/`shell_exec` concurrency and lifecycle belong
to the Agent Execution ADR. If a future separate executor process is added,
it remains subordinate to the same Pane scope and must not become a second
Session-state authority.

This decision is selected over one worker with N PTYs because:

- a Pane worker crash does not destroy unrelated Panes;
- resource attribution and cleanup are naturally per-Pane;
- floating and agent-created Panes have independent lifecycle boundaries;
- observer and Attach routing can target a Pane directly.

The cost is more worker processes, more sockets, and a more complex qwed
inventory/recovery path. That cost is accepted as the price of per-Pane
isolation and resource ownership.

### 4. ChannelProfile and AuthContext

Introduce `ChannelProfile` as an authorization and projection boundary, not as
a second state owner.

Initial profiles are:

| Profile | Purpose |
|---|---|
| `human_interactive` | Attach, input, resize, menu, and authorized control |
| `human_observer` | Read-only stream/snapshot subscription |
| `agent_exec` | Structured command execution and command handles |
| `agent_stream` | Structured output/lifecycle subscription |
| `agent_observer` | Read-only event or output observation |

Each channel targets a Session, Pane, or Task. The initial `human_interactive`
and `human_observer` channels target a Pane. Interactive Attach authority is
therefore Pane-scoped, while the Lease remains Session-scoped for Session-wide
lifecycle control. `human_interactive` uses the current owner and Lease
generation as fencing inputs. `human_observer` is authorized by its profile and
AuthContext and never acquires a write Lease or mutation authority.

`human_observer` is a qwed-owned read-only projection over an exclusive
internal worker Attach channel. The worker is not required to implement a
native `read_only` Attach mode. qwed rejects observer input, resize, lifecycle,
and control attempts before forwarding anything to the worker, and an observer
must never acquire write authority.

Every profile carries:

- an opaque `AuthContext`;
- an output projection (`render` or `structured`);
- an operation/capability allowlist;
- a backpressure and replay policy;
- a transport binding (Attach, control, or event subscription).

`AuthContext` is deliberately opaque in this ADR. It must not be hard-coded as
a flat token or a username. The security ADR will define whether it contains a
principal reference, an epoch chain, a token handle, or another verifiable
form. This reservation prevents the channel boundary from being designed
around a simpler identity model and then rewritten for nested tenants.

### 5. Minimum Principal object

Add a first-class `Principal` record as the shared identity anchor for human
operators, agent tenants, and nested agent principals:

```text
Principal
  id: opaque stable id
  parent_id: optional Principal id
  epoch: monotonic integer
  revoked_at: optional timestamp
```

`Principal` records are tombstoned, never hard-deleted while descendants may
exist. The epoch is the revocation generation used by the security layer.
This ADR does not define token encoding, severity policy, or the complete
ancestor-check algorithm; those belong to the Agent Security ADR.

### 6. Explicit non-decisions

This ADR does not decide:

- `exec` versus `shell_exec`;
- command-handle shape, timeouts, or wait semantics;
- capability-token encoding, token transport, severity policy, or freeze/kill
  defaults;
- sandbox, namespace, UID, filesystem, or network isolation;
- Pane geometry, compositor, copy-mode, or floating-overlay behavior;
- scrollback persistence;

Those decisions may depend on this ADR, but must not be inferred from it.

Sandbox is explicitly outside QWE core for this stage. Prefer existing
third-party isolation tools and wrappers for early use; a caller can launch
the third-party sandbox as the program inside a normal Pane. QWE resource
limits still apply to the resulting process tree, but QWE does not claim
filesystem, network, UID, or namespace isolation. An optional external sandbox
adapter may be considered later through a separate ADR; it is not a shared
prerequisite and is not required for this ADR's acceptance.

### 7. Delivery order

Stage 7 Linux Resource Governance is the next implementation milestone. After
the shared prerequisites, Session Layout is the default next feature track.
Agent Execution may start in parallel only when its own ADR is accepted, the
shared Pane/Task and channel contracts are used unchanged, and staffing allows
both tracks to remain independent.

## Consequences

- The Session is no longer synonymous with exactly one worker process.
- qwed worker inventory, recovery, and health reporting must address Pane
  workers.
- Interactive attachment and observer authority are Pane-scoped; Session-wide
  Lease authority remains the fence for lifecycle operations.
- Storage grows Pane cardinality beyond one Pane per Session and adds
  Principal and ResourceScope records. The v10-to-v11 migration plan in
  [Storage Migration](../storage-migration.md) preserves existing V0 Sessions,
  attachment authority, idempotency records, and task history.
- Resource policy is expressed against a scope target, not directly against a
  cgroup path.
- Human and agent channels share the same authoritative state; only profile,
  projection, and authorization differ.
- Existing V0 behavior remains the one-Pane/one-worker special case until the
  migration is implemented.

## Alternatives Considered

### One worker per Session with N PTYs

This would reduce process count and keep one worker identity per Session. It
was not selected because a worker crash would destroy every Pane in the
Session, and per-Pane resource attribution and cleanup would require a second
layer inside one process. It remains a possible performance optimization only
through a later superseding ADR.

### Keep per-Session cgroups and add Pane data later

This is initially simpler, but it would make resource policy, floating
programs, and agent command budgets depend on a scope model that cannot
represent their ownership. It was not selected.

### Separate human and agent runtimes

This would allow faster feature-specific iteration but would create two
Session/Pane/PTY state paths. It is rejected by the single-kernel invariant.

### ChannelProfile without AuthContext

This would assume a flat identity and force a channel-boundary rewrite when
capability tokens and nested tenants arrive. It was not selected.

## Acceptance and Follow-up

Accepted on 2026-11-09 after the Stage 6.4 evidence in Experiment 0028. The
accepted contract is:

1. the ResourceScope/ResourceBackend boundary with Pane-level first
   enforcement;
2. one worker per Pane, with inventory and recovery addressed by Pane;
3. ChannelProfile, Pane-targeted Attach, the qwed-owned observer projection,
   and the opaque AuthContext reservation;
4. the minimum Principal object and tombstone rule;
5. complete resource-scope teardown, including detached process groups;
6. sandboxing as an external integration rather than a QWE core prerequisite.

Stage 6.4 and Stage 6.5 are closed. The design baseline, technical
architecture, IPC contract, storage migration plan, test matrices, roadmap,
and development plan were updated with this decision. Implementation may start
under Stage 7; it does not authorize the full Session Layout or Agent Execution
tracks.

The Agent Execution ADR must then define:

- `exec` versus `shell_exec`;
- command handles and wait/timeout semantics;
- shared PTY foreground-process-group access;
- event stream, observer, and backpressure;
- capability-token, epoch-chain, severity, and freeze/kill policy.
