# Agent Runtime and Terminal UX Feature Assessment

Status: Assessment complete for this review round; shared prerequisites
accepted; remaining policy decisions open
Date: 2026-11-09

This document evaluates the agent-native features proposed for QWE together
with three human-facing terminal UX features:

1. pane-scoped direct copy without entering copy mode for visible selections;
2. free-dragging floating terminal programs over a scrollable session;
3. a terminal-local quick menu for actions such as enabling core dumps and
   machine inspection.

The assessment uses four verdicts:

- **Accept**: the direction fits QWE and can be planned.
- **Accept with redesign**: the goal is useful, but the proposed mechanism
  conflicts with the current architecture or security model.
- **Defer**: useful, but blocked on another capability or too expensive for
  the next milestone.
- **Reject**: should not enter QWE core in the proposed form.

An **Accept** verdict is not implementation authorization. It does not change
Stage 6, Stage 7, Stage 8, or the supported platform matrix. A feature becomes
committed only through an accepted ADR and an updated development plan.

## 0. Closure Status

This document has two different closure questions:

| Question | Verdict |
|---|---|
| Is the assessment/review round complete? | **Yes.** The feature set, conflicts, dependencies, and decision points are now explicit enough to stop the review loop. |
| Is the implementation plan ready to be finalized? | **No.** The shared prerequisites are accepted, but several agent-security and execution-policy choices remain open. |

### 0.1 Resolved in this review round

- The product is not "human runtime versus agent runtime". It is one shared
  kernel with human and agent channel profiles.
- Stage numbering must not be rewritten. New work uses stable names
  (`Resource Governance`, `Session Layout`, `Agent Execution Model`,
  `Terminal UX and Diagnostics`) and an explicit mapping.
- `ResourceScope` / `ResourceBackend` is accepted and makes Pane-first
  resource attribution the next implementation target.
- `RevocationEpoch` is the selected single-level cascade mechanism: increment
  the principal/tenant epoch and reject older tokens; no token graph
  traversal. Nested-tenant option A is recommended but still needs ADR
  acceptance.
- Human lock defaults are split by revocation reason: administrative
  revocation preserves running work, security suspension freezes by default,
  and termination is explicit.
- `AR-002` (`wait_idle`) and `AR-007` (command handle) must be delivered in
  the same ADR.
- Scrollback persistence is now an explicit cross-cutting conflict, not an
  implied part of basic copy.
- Human/agent priority is a scheduling decision after shared prerequisites,
  not a product-identity declaration.
- The shared-prerequisites decision gate accepted `ResourceScope`,
  one-worker-per-Pane topology, `ChannelProfile`, the opaque `AuthContext`,
  the minimum `Principal` tombstone model, and the external-sandbox boundary
  in [ADR 0021](adr/0021-shared-prerequisites.md).
- Stage 7 is the next implementation track. Session Layout is the default next
  feature track; Agent Execution may run in parallel only with its own
  accepted ADR and sufficient staffing.

### 0.2 Recommended resolutions from the deep review

These recommendations still require their own accepted contracts:

- Use real-time ancestor epoch checks for nested tenants (option A below).
- Extract PTY foreground-process-group lookup as one internal primitive shared
  by wait, blocked-input detection, and shell signal delivery.
- Limit the first copy feature to the visible screen plus the existing
  in-memory 2000-line scrollback; defer disk-backed scrollback to AR-012.
- Make unspecified revocation severity fail safe to freeze; require a
  high-trust principal to mark a revocation as administrative; give freeze a
  timeout and escalation policy.
- Keep sandbox external: prefer third-party isolation tools in early versions;
  do not make a core `SandboxBackend` a prerequisite.

### 0.3 Implementation-plan closure

The prior blockers are resolved:

1. Nested tenants use real-time ancestor epoch checks.
2. Revocation severity fails safe to freeze, requires high-trust authorization
   for administrative revocation, and escalates after a finite grace period.
3. Fresh-process `exec` is selected first; `shell_exec` is deferred. Command
   handle and `wait_idle` are one delivery.
4. Copy includes the existing bounded in-memory scrollback; persistence remains
   deferred.

These decisions are recorded in
[ADR 0032](adr/0032-agent-execution-model.md).

### 0.4 Stage conclusion

The feature-assessment, conflict-analysis, and Agent Execution decision stage
are complete. Implementation planning may proceed under ADR 0032.

### 0.5 Stage 6.5 disposition

Stage 6.5 is complete. ADR 0021 accepts Pane-first ResourceScope, one worker
per Pane, Pane-keyed inventory and Attach, the qwed-owned observer projection,
complete scope teardown, ChannelProfile with opaque AuthContext, and the
minimum Principal tombstone. Schema v11 invariants are recorded in
[Storage Migration](storage-migration.md). This acceptance does not resolve
nested-tenant security, revocation severity, `exec` semantics, or copy
persistence.

## 1. Current Architecture Constraints

| Constraint | Current state | Consequence for this assessment |
|---|---|---|
| Authority | `qwed` is the sole authority for Session, Task, Lease, worker inventory, and persistent state | Agent channels must terminate in qwed or a qwed-controlled worker path, not a second daemon |
| Worker ownership | V0 has one worker per Session. ADR 0021 accepts one worker per Pane after schema v11 | Pane-scoped features must use the accepted Pane topology and cannot retain Session-keyed worker assumptions |
| Object cardinality | V0 has one Pane and one TaskInstance per Session. Schema v11 removes the uniqueness assumption | Pane-scoped copy and floating panes still require their own layout/storage decisions and implementation |
| Attach | Interactive Attach remains exclusive; `read_only` opens a qwed-owned observer alongside one interactive writer | `human_observer` is implemented; agent stream/observer profiles reuse the same projection rules under ADR 0032 |
| IPC | Unix domain socket, length-prefixed JSON | Do not add gRPC, SSE, or a TCP listener for local operation |
| Credentials | Secrets are delivered through a broker and inherited descriptor 3, never argv/environment/logs/SQLite | Per-command credential injection must preserve the descriptor model |
| Resource control | Linux cgroup v2 limits resource usage; it is not a sandbox | Do not describe cgroups as filesystem, network, process, or UID isolation |
| Journal | The journal is diagnostic-only and may start mid-ANSI/mid-UTF-8 | Semantic history and replay need a separate event/capture contract |
| Scrollback | Workers configure 2000 lines of `vt100` scrollback in memory | A minimal history view is feasible, but persistence and protocol exposure are not implemented |
| Product scope | Stages 7 and 8 are complete; ADR 0032 accepts the Agent Execution Model | Agent execution reuses the shared Pane, Task, ResourceScope, and channel contracts rather than extending resource governance |

### 1.1 Product framing

QWE should not be described as "a human runtime or an agent runtime". The
shared core is a resource-managed terminal kernel:

```text
                 qwed authority
                      |
        +-------------+-------------+
        |                           |
  human channel                agent channel
  attach/input/resize          exec/stream/control
  menu/observer                observer/event wait
        |                           |
        +-------------+-------------+
                      |
          Session / Pane / PTY / Task
          Lease / resource policy / journal
```

Human and agent are **channel profiles over the same kernel**, not two
independent runtimes. They must share one authoritative Session/Pane/PTY
state; only their projections, authorization, and interaction shapes differ.

The planning question is therefore not "which runtime wins", but "which
channel profile is first-class in the next milestone, and when does agent
support leave E05 research". A feature can be useful to both channels while
belonging to different delivery tracks.

This framing is an **architecture decision**, not a product-identity choice.
The shared prerequisites are `ResourceScope`, `WorkerTopology`, and
`ChannelProfile`; they are now accepted in ADR 0021. The human compositor and
agent execution tracks may run in parallel with sufficient capacity; track
priority is a scheduling decision, not a declaration that QWE is "for humans"
or "for agents". The agent track may start before the full human compositor,
but it must reuse the same kernel and the defining interfaces. It must not
bypass worker topology or channel policy, and it must not create a second
Session/Pane state path.

## 2. Assessment Summary

| ID | Feature | Verdict | Proposed track | Primary conflict or dependency |
|---|---|---|---|---|
| AR-001 | Shell integration and command boundaries | Accept with redesign | E05 | Shell-specific; in-band markers are not a security boundary |
| AR-002 | `wait_idle` / `wait_exit_code` | Accept with redesign | E05 | Idle is not command completion; exit code needs a command boundary |
| AR-003 | Detect blocked-on-input state | Defer | E05 | `/proc`/`wchan` is a heuristic, Linux-only, and not a semantic guarantee |
| AR-004 | Structured output stream | Accept with redesign | E05 | Must use Unix-socket framing, not SSE |
| AR-005 | Split stdout and stderr | Reject in PTY; accept only as AR-009 | E05 | PTY merges descriptors 1 and 2; source cannot be recovered at the proxy |
| AR-006 | Token-aware output summary | Accept with redesign | E05 | Must preserve raw output and mark truncation |
| AR-007 | Exec RPC and command handle | Accept with redesign | E05 | Must distinguish one-shot exec from persistent shell execution |
| AR-008 | Event subscription and push | Accept with redesign | E05 | Needs sequence numbers, replay cursors, and per-subscriber backpressure |
| AR-009 | Non-TTY exec mode | Accept | E05 | Changes `isatty` behavior and loses interactive TUI support |
| AR-010 | Agent execution profile | Accept with redesign | E05 | Current create path has no cwd/env/profile; env must be allowlisted |
| AR-011 | Output redaction | Accept with redesign | E05 | Best-effort only; must cover agent output and persisted logs |
| AR-012 | Semantic execution history | Defer | E05 | Large data; separate from the diagnostic journal; external retention |
| AR-013 | Read-only observer | Accept with redesign | E05 | Current V0 rejects read-only Attach |
| AR-014 | Cross-pane signals | Defer | E05+ | Quickly becomes orchestration; not a terminal primitive |
| AR-015 | Out-of-band control/signal API | Accept with redesign | E05 | Current control exists, but there is no generic signal verb or priority path |
| AR-016 | Pane ownership and multi-agent isolation | Accept with redesign | E05 | Authorization is not OS isolation; current Lease is control authority |
| AR-017 | Human-root / agent-tenant capability model | Accept with redesign | E05 | Same-UID peer credentials cannot distinguish two agents |
| AR-018 | Per-command credential injection | Defer | E05 | Needs a command-level wrapper and descriptor inheritance; no env injection |
| AR-019 | Sandbox tiers | Defer; prefer external third-party tools | External | cgroup resource limits are not a sandbox; QWE core should not implement isolation |
| AR-020 | Kernel plus multiple channels | Accept | E05 | Maps well to qwed plus worker; do not create a second daemon |
| AR-021 | pgvector integration | Reject from core | External | Export JSONL/events; external systems own analytics storage |
| HX-001 | Direct pane-scoped mouse copy | Accept with redesign | Stage 8 | Native terminal selection cannot be pane-clipped in a composited screen |
| HX-002 | Free-drag floating program overlay | Accept with redesign | Post-Stage 8 | Conflicts with one-worker/one-PTY and one-Pane-per-Session assumptions |
| HX-003 | Terminal quick menu | Accept | Stage 8 TUI | Menu is client UI; actions still need protocol and authorization |
| HX-004 | Per-session core-dump toggle | Accept only after security review | Stage 8+ | Core dumps can contain secrets; global `core_pattern` is privileged |
| HX-005 | Host/machine management | Defer | Separate privileged feature | Expands QWE from task runtime into host administration |

## 3. Agent Runtime Features

### 3.1 Command lifecycle

#### AR-001: Shell integration and command boundaries

Verdict: **Accept with redesign**

The goal is correct: an agent should not have to infer command start, command
end, or exit code from a prompt regex or a fixed sleep. OSC 133-style shell
integration is a reasonable UX mechanism.

Required constraints:

- It must be opt-in per execution profile, not silently injected into every
  shell.
- It must support at least the shells QWE claims to support; unsupported
  shells must fall back to explicit `unknown` rather than a false boundary.
- It must define behavior for nested shells, `sudo`, `ssh`, containers, and
  REPLs. Those environments may lose the marker stream.
- Command boundaries and process exit are different events. A shell command
  can return while a background child remains alive.
- In-band OSC markers can be printed by an arbitrary program. They are useful
  for UX and automation, but not a security boundary. Prefer an out-of-band
  integration descriptor where available.

Conflict: the current creation path accepts a program and argv but does not
install a shell wrapper or a shell-integration profile.

Open questions:

- Which shells are in the first support matrix?
- Is shell integration part of the Session or of each command?
- How are background jobs, pipelines, and shell built-ins represented?

#### AR-002: `wait_idle` and `wait_exit_code`

Verdict: **Accept with redesign**

The useful primitives are:

- wait for a command handle to finish;
- wait for a process or process group to exit;
- wait until a subscriber cursor advances;
- wait for an explicit `idle` event.

`wait_idle` must not be documented as command completion. No output for N
milliseconds can mean a compiler is busy, a program is sleeping, a network
read is blocked, or a process is waiting for input. It should be a separate
heuristic event with a confidence/`unknown` state.

Conflict: there is no command handle today. `TASK_STOP` and `SESSION_KILL` are
lifecycle operations, not command-boundary APIs.

AR-002 and AR-007 must be designed in the same ADR. Wait semantics without a
stable command identity produce ambiguous APIs: it is impossible to say what
`wait_exit_code` refers to, or which timeout cancels which process group.

AR-002, AR-003, AR-007, and shell signal delivery share one low-level
primitive: query the PTY foreground process group, for example with
`tcgetpgrp`/`TIOCGPGRP`. Implement that once as an internal runtime capability.
Do not let each feature independently guess from `/proc` or synthesize
`Ctrl-C` bytes.

For `exec`, exit status comes from `waitpid` and is deterministic. For
`shell_exec`, missing shell-integration markers must produce an explicit
`unknown` result; do not invent a command boundary. If a worker can host
multiple concurrent non-PTY `exec` children, that concurrency model depends on
the still-open `WorkerTopology` decision.

#### AR-003: Detect blocked-on-input state

Verdict: **Defer**

Reading `/proc/<pid>/status` and `wchan` is useful as a diagnostic hint, but it
is not portable or exact:

- macOS has no `/proc`;
- `wchan` is kernel-version dependent and may be restricted;
- a process blocked in `read` may be reading a pipe, socket, or file rather
  than the terminal;
- a foreground process group can be waiting at a shell prompt without a
  simple `wchan` signature.

Expose this only as `suspected_waiting_input` or `unknown`, never as a
guaranteed state. The reliable path is command-level wrapper plus terminal
foreground-process-group observation.

### 3.2 Output semantics

#### AR-004: Structured output stream

Verdict: **Accept with redesign**

The stream should be a versioned event stream over the existing Unix socket,
not SSE. A frame should carry at least:

```text
event_id, sequence, session_id, pane_id, command_id, stream,
timestamp, encoding, payload, truncated
```

The stream must define replay/retention and what happens to a slow subscriber.
Do not let event publication block the PTY reader.

Conflict: the current Attach OUTPUT queue is per-client and bounded, but it is
not a general event subscription contract.

#### AR-005: Split stdout and stderr

Verdict: **Reject in PTY mode; accept only as part of AR-009**

This is the most important technical correction in the source proposal. In a
normal PTY, descriptors 1 and 2 point to the same PTY slave. Once bytes reach
the PTY master, QWE cannot know whether a byte came from stdout or stderr.

Options:

- **PTY mode**: expose one merged stream and preserve terminal behavior.
- **Non-TTY exec mode**: use separate pipes for stdout and stderr. This gives
  stream attribution and exit codes, but loses terminal/TUI behavior and
  changes `isatty` semantics.

Splitting after the PTY proxy is not implementable without changing the
execution model. Separate pipes also lose the original interleaving order,
which must be documented.

#### AR-006: Token-aware output summary

Verdict: **Accept with redesign**

Head+tail truncation, error-line extraction, and deduplication are useful
agent-facing projections. The daemon must not silently replace the raw
output. The response should include:

- total bytes and line counts;
- whether truncation occurred;
- the strategy used;
- a raw-output handle or artifact path;
- the first and last retained regions.

Summary generation must be deterministic. Do not make the kernel call an LLM.

### 3.3 Execution channels

#### AR-007: Exec RPC and command handle

Verdict: **Accept with redesign**

Define two distinct execution modes:

| Mode | Semantics | Stream | Shell state |
|---|---|---|---|
| `exec` | one process, argv array, one-shot | separate or merged pipes | none |
| `shell_exec` | command in an existing shell | PTY merged stream | persistent |

`exec` should accept argv, not only a shell string, to avoid quoting ambiguity.
`shell_exec` needs shell integration for command boundaries and must preserve
the existing shell's aliases, functions, and cwd.

A command handle should support status, wait, timeout, cancellation, and
reconnect. Timeout must kill the intended process group, not an unrelated
process.

#### AR-008: Event subscription and push

Verdict: **Accept with redesign**

Use a local subscription channel with:

- monotonic sequence numbers;
- replay from a cursor;
- a bounded per-subscriber queue;
- explicit slow-subscriber policy;
- separate output, lifecycle, resource, and policy event classes.

The current Attach channel already demonstrates snapshot-plus-stream
continuity, but it is not a general event bus. Do not reuse the 1 MiB Attach
queue for all event types without a new contract.

#### AR-009: Non-TTY exec mode

Verdict: **Accept**

This is the clean solution for agents that need machine-readable output and
exit codes. It should be explicit:

```text
exec(..., tty=false)
```

The result must state that `isatty` is false and that interactive programs may
fail or change output. It must not be silently used for commands that expect a
terminal.

### 3.4 Agent environment and output policy

#### AR-010: Agent execution profile

Verdict: **Accept with redesign**

A profile should include cwd, environment allowlist, shell integration mode,
timeout, resource policy, and terminal mode. Current `SESSION_CREATE` has
program, args, credential, and resource policy, but no cwd/env/profile.

Security requirements:

- environment values must be allowlisted or explicitly supplied by the
  operator;
- do not pass secrets through environment;
- profile changes must be auditable;
- a profile must not silently change an existing shell's cwd or environment.

#### AR-011: Output redaction

Verdict: **Accept with redesign**

Redaction is valuable, but it is best-effort:

- known secret values can be redacted;
- encoded, split, compressed, or transformed secrets may be missed;
- a command can print a derived secret that is not an exact registry match.

Apply redaction at every egress that can leak:

- agent event stream;
- persisted execution log;
- copied output;
- diagnostic/export surfaces.

Define whether human observer output is redacted separately from agent
output. Do not claim that redaction makes arbitrary command output safe.

### 3.5 History and observation

#### AR-012: Semantic execution history

Verdict: **Defer**

The idea is sound: store command, timestamps, exit code, output summary, and
resource events separately from raw terminal output. Do not put full
stdout/stderr into SQLite, and do not use the diagnostic journal as the
history source.

A future design should use a bounded append-only event log with retention,
redaction, and export. Full-screen terminal replay requires a separate
gap-aware capture contract.

#### AR-013: Read-only observer

Verdict: **Accept with redesign**

V0 explicitly rejects `read_only` Attach. Do not implement observer by
weakening the exclusive Attach contract. Prefer a separate subscription
channel that:

- does not acquire write authority;
- cannot send input or resize;
- has its own queue and disconnect policy;
- is authorized by a capability token;
- can be enabled by a human for an agent pane and disabled by default for a
  human/private pane.

### 3.6 Control and multi-agent security

#### AR-014: Cross-pane signals

Verdict: **Defer**

A signal bus is easy to turn into an orchestration engine. Keep it out of the
core until there is a separate ADR defining:

- signal ownership;
- delivery guarantees;
- replay;
- failure semantics;
- how a signal differs from a command.

#### AR-015: Out-of-band control and signal API

Verdict: **Accept with redesign**

QWE already separates control requests from raw PTY input, which is a good
start. Add an explicit signal verb for `SIGINT`, `SIGTERM`, and `SIGKILL`
instead of requiring `send-keys C-c`.

The control path must not wait behind a long-running command request. Verify
worker control concurrency and define a priority path for interrupt, kill,
and resize.

For `shell_exec`, signal delivery should target the PTY foreground process
group rather than injecting a synthetic `Ctrl-C` byte into the terminal
stream. This is the same foreground-process-group primitive used by AR-002 and
AR-003.

#### AR-016: Pane ownership and multi-agent isolation

Verdict: **Accept with redesign**

Separate three concepts:

| Concept | Purpose |
|---|---|
| Resource policy | CPU, memory, task count |
| Authorization | Which agent may read/write/control a Session or Pane |
| Sandbox | Filesystem, network, UID, namespace isolation |

Current Leases are control-authority records with logical owner identities.
A pane write lease can extend that model, but it must not be confused with OS
isolation. Two same-UID agents can still read each other's files and signal
each other's processes unless a sandbox or separate UID is used.

#### AR-017: Human-root and agent-tenant capability model

Verdict: **Accept with redesign**

The hierarchy is sound:

```text
human operator (root authority)
  -> capability token
     -> agent (scoped tenant)
```

Current same-UID peer checks are not enough to distinguish a human from an
agent. Add an opaque capability token issued by the human/qwed side, scoped to
operations, Sessions, panes, commands, and resources. Do not put the token in
environment; use an inherited descriptor or a protected local socket.

Tenant suspension must cascade to every token issued under that tenant. Use a
monotonic `revocation_epoch` on each principal/tenant. A token records its
`principal_id`, `issued_epoch`, scope, and expiry. Authentication requires the
token's `issued_epoch` to equal the principal's current `revocation_epoch`.
Suspension increments the principal's epoch, so every older token fails
without enumerating tokens or traversing a parent graph.

The parent/tenant relation is still useful for audit, ownership, and
displaying who issued a token, but it is not the cascade mechanism. In-flight
requests must be fenced by the same epoch; otherwise a suspended tenant can
complete already-queued work with an old token. If nested tenants are
supported, the ADR must decide whether a token references the root human
principal or whether auth checks an ancestor epoch chain.

Nested-tenant options:

| Option | Mechanism | Correctness | Cost |
|---|---|---|---|
| A. Real-time ancestor chain | Token names a principal; auth walks parent links and checks every current epoch | Immediate revocation at any ancestor | O(depth) lookups and a persisted tenant tree |
| B. Snapshot ancestor epochs at issue | Token embeds ancestor epochs | Ancestor revocation after issue does not take effect until expiry | Cheap auth, weak revocation |
| C. Global revocation epoch | Any revocation increments one global counter | Invalidates unrelated agents | Unacceptable collateral invalidation |
| D. Top-level tenant epoch | One epoch per human/root tenant | Revokes unrelated agent trees under the same human | Coarse but bounded |

Recommended: **option A** for this single-host Unix-socket runtime. A depth of
2-4 parent lookups is negligible compared with command execution, and it
preserves immediate cascade revocation without a TTL workaround.

The `Principal` object should be first-class: `id`, optional `parent_id`,
monotonic `epoch`, and optional `revoked_at`. Do not hard-delete a principal
with descendants; use a tombstone so ancestor-chain lookup remains defined.

A token revocation is not a process-lifecycle operation. Define a separate
reason/severity policy that decides whether running tasks continue, freeze, or
terminate. See Section 5.9.

#### AR-018: Per-command credential injection

Verdict: **Defer**

The current design loads a credential for a Session and delivers it through
descriptor 3. Per-command injection needs a command-level wrapper:

- `qwed` resolves the credential name;
- the wrapper receives the secret through a broker/FD;
- the child sees only the inherited descriptor and the credential-FD
  environment variable;
- the persistent shell's environment is not modified.

For an already-running shell, there is no portable way to change its
environment for one command. The agent must invoke a wrapper or a dedicated
command channel.

#### AR-019: Sandbox tiers

Verdict: **Defer; prefer external third-party integration**

Follow-up: [ADR 0033](adr/0033-bubblewrap-sandbox-adapter.md) accepts Bubblewrap
as the first Linux external backend.

`normal / sandboxed / network-isolated` is a major security feature, not a
variant of resource policy. It requires an ADR covering:

- filesystem/workspace boundaries;
- user/UID or namespace model;
- network policy;
- process visibility;
- credential handling;
- platform differences;
- human approval.

Do not claim that cgroup limits provide this isolation.

Early QWE should prefer existing third-party isolation tools: the caller can
run the third-party sandbox as the Pane program, while QWE applies resource
scope separately. QWE core should not implement namespaces, filesystem
boundaries, network policy, or credential isolation in this stage. A future
optional adapter must be a separate ADR and must not be described as a core
sandbox.

### 3.7 Architecture and integration

#### AR-020: Kernel plus multiple channels

Verdict: **Accept**

This is the strongest architectural idea in the source proposal. Map it to
QWE as follows:

| Channel | QWE implementation |
|---|---|
| Exec | new qwed command-execution operation, worker wrapper |
| Stream | existing Attach OUTPUT plus a structured event subscription |
| Raw input | existing `SESSION_INPUT` and Attach INPUT |
| Control | existing lifecycle operations plus explicit signal/priority verbs |
| Observer | new read-only subscription, not read-only Attach |

`ChannelProfile` must include an opaque `AuthContext` extension point. The
shared-prerequisites ADR does not need to finalize capability-token internals,
but it must not assume a flat identity that can be replaced later without
touching the channel boundary. The security ADR will define whether
`AuthContext` carries a principal, an epoch chain, or another verifiable
credential form.

QWE does not need a second daemon. qwed remains the authority, and the worker
remains the PTY owner.

#### AR-021: pgvector integration

Verdict: **Reject from core**

QWE should emit JSONL/events and let Bug KB, ANFIM, or another external system
choose its storage. Adding pgvector to QWE core introduces a database and
analytics dependency that does not belong in the terminal runtime.

## 4. Human Terminal UX Features

### HX-001: Direct pane-scoped mouse copy

Verdict: **Accept with redesign**

The requested behavior is:

- select visible text with the mouse without first entering copy mode;
- keep the selection inside the pane where the drag started;
- never accidentally include a neighboring pane on the same screen row;
- copy to the system clipboard and show feedback.

This is a better default than tmux's global mouse capture, but it has a hard
architectural constraint: once panes are composited into one client screen,
the outer terminal's native selection cannot know pane boundaries. If QWE
does not capture the mouse, native selection can include the border, status
bar, or another pane.

Recommended model:

| Mouse policy | Behavior |
|---|---|
| `off` | QWE never captures; use the outer terminal's native selection |
| `copy` | QWE captures only when selecting/copying; drag is clipped to the origin pane; release copies |
| `on` | QWE captures globally, similar to tmux mouse mode |

The default should be `copy` once the compositor exists:

- hit-test the pane under the drag start;
- clamp the selection rectangle to that pane;
- drag beyond the pane boundary must clamp or auto-scroll, not select the
  neighbor;
- release copies to the system clipboard;
- double-click selects a word, triple-click selects a line;
- `Shift`/`Option` can bypass to native terminal selection where the terminal
  supports it;
- outside selection, mouse events pass to the application when the
  application requested mouse mode.

This does not require a full copy-mode UI. Copy-mode remains useful for
history, search, and keyboard-only selection. The user-facing path should not
force `prefix [` just to select visible text.

Architecture conflicts:

- V0 has one Pane per Session, so this only matters after Stage 8.
- The current Attach channel carries one rendered stream; pane clipping needs
  pane geometry and a compositor.
- The current terminal has no system-clipboard capability; OSC 52,
  `pbcopy`, `wl-copy`, and `xclip` need a capability/fallback contract.
- QWE must choose between native selection (no pane clipping) and captured
  selection (pane clipping, app mouse arbitration, clipboard code).

Open questions:

- Should a drag that starts in one pane and ends in another select the origin
  pane only, or both panes? Recommendation: origin pane only by default.
- Does copy include wrapped lines and wide characters correctly?
- Should selection include scrollback? If yes, it is a history feature, not
  only a visible-screen feature.

### HX-002: Free-drag floating program overlay

Verdict: **Accept with redesign, post-Stage 8**

The target is a floating terminal program such as `htop` or `nvidia-smi` that
can be moved freely over a scrollable command-line pane. This is not tmux's
current floating-pane behavior; it is a compositor feature.

Suggested phasing:

1. fixed popup pane: centered, non-draggable, one overlay;
2. draggable overlay: geometry, z-order, focus, resize;
3. multiple overlays: stacking, pinning, minimize, close confirmation.

Recommended object model: reuse `Pane` with `kind=normal|floating`, geometry,
and z-index. Do not create a separate `Overlay` object until a real need
appears. A floating `htop` is still a terminal process with a PTY.

Required behavior:

- focus switches between the overlay and the base pane;
- keyboard input goes to the focused pane;
- mouse clicks/drags go to the floating pane under the cursor;
- wheel scrolling targets the hovered pane; the base pane remains scrollable
  while the overlay is visible;
- the overlay has its own process tree and resource budget;
- close requires confirmation or an explicit `pin`/`locked` state;
- closing the overlay kills only its process tree and leaves the base Session
  alive;
- detach/attach must define whether overlays persist and where they appear.

Architecture conflicts:

- V0 D053 assigns one worker and PTY to a Session; the accepted target assigns
  one worker and PTY to each Pane, including a floating Pane.
- `panes.session_id` is currently `UNIQUE`; schema v11 removes that constraint
  before multi-Pane implementation.
- The current Attach stream is single-screen; a compositor must render
  overlapping panes and route input by geometry.
- Per-pane resource scope conflicts with the current per-Session cgroup scope.
- Close protection must not rely on the client alone; qwed owns lifecycle and
  must know whether an overlay is pinned or disposable.

Open questions:

- Is an overlay a Pane, a Window, or a new object?
- Does a floating program survive detach?
- Does it consume the same Session resource budget or its own?
- Does closing the client close overlays, or only hide them?

### HX-003: Terminal quick menu

Verdict: **Accept**

A terminal-local menu or command palette is a client-side TUI feature. It does
not need a second PTY. It can be implemented as an overlay drawn by the QWE
client, with actions sent as control requests.

Candidate menu groups:

- Session: list, attach, detach, rename, close;
- Pane: split, focus, resize, zoom, copy history;
- Resources: current usage, limits, violations;
- Diagnostics: version, capabilities, logs, host summary;
- Safety: core-dump toggle, confirm/deny dangerous actions.

The menu must:

- open and close without sending its keys to the PTY;
- be keyboard-first and optionally mouse-selectable;
- mark dangerous actions and require confirmation;
- evaluate authorization at the qwed/control boundary, not only in the menu;
- remain usable when a program is in alternate-screen mode or has mouse mode
  enabled.

Architecture conflict: QWE has no TUI menu layer today, and machine-management
operations do not exist in the control protocol. The menu framework can be
client-only; the actions need separate protocol work.

### HX-004: Per-session core-dump toggle

Verdict: **Accept only after a security review**

The phrase "inject core dump" is ambiguous. This assessment assumes it means
"enable core dumps for a Session or Task for debugging". If it means
"inject/analyze an existing core dump", that is a separate feature.

Constraints:

- prefer per-process or per-Session `RLIMIT_CORE` at spawn;
- changing a running process needs an explicit and platform-specific
  mechanism; do not silently apply it to unrelated processes;
- global `core_pattern` changes are privileged and affect the host; do not
  expose them through an ordinary agent menu;
- core dumps can contain credentials, tokens, memory, and private data;
- storage location, permissions, retention, encryption, and deletion must be
  explicit;
- enabling core dumps must be opt-in, auditable, and visibly marked in status.

Architecture conflict: QWE does not currently expose core-dump policy in
Session/Task records or platform capability reporting. This is a new security
surface, not a UI-only feature.

### HX-005: Host and machine management

Verdict: **Defer; likely reject from core as a privileged feature**

"Machine management" is too broad. Split it:

- read-only host information (CPU, memory, disk, load, QWE version);
- Session/resource inspection;
- process management;
- service management;
- global kernel or core-dump settings.

Read-only host information can be a menu action. Process/service/global
settings require a separate privileged design, platform-specific permissions,
and a security ADR. Do not turn QWE into a host administration daemon through
a terminal menu.

## 5. Cross-Cutting Conflicts

### 5.1 One worker per Session versus multiple panes

D053 reserves one worker per Session for V0, owning one PTY, process group, VT
state, and journal. ADR 0021 accepts one worker per Pane with Pane-keyed
inventory, health, recovery, Attach routing, and ResourceScope. V0 behavior
remains unchanged until schema v11 and the multi-Pane protocol are implemented.

### 5.2 Pane uniqueness and geometry

The current storage schema enforces one Pane per Session and stores no
geometry, z-index, or focus state. Schema v11 removes the cardinality
constraint. HX-001, HX-002, and any pane-scoped copy feature still require a
geometry/compositor contract.

### 5.3 Exclusive Attach and observers

The current V0 Attach model is exclusive, and `read_only` is reserved but
rejected. ADR 0021 accepts `human_observer` as a qwed-owned read-only
projection over an exclusive internal worker channel. Agent observer channels
still require the capability and queue policy from the Agent Security ADR.

### 5.4 Secrets and environment

The current contract forbids secrets in environment, argv, logs, and
persisted state. Any per-command credential design must use the broker and
inherited descriptor model. A menu action must not set a secret in a shell's
environment as a shortcut.

### 5.5 Resource limits versus isolation

Stage 7 cgroup work is resource governance. It does not isolate filesystems,
networks, processes, or UIDs. Multi-agent security needs a separate sandbox
design, preferably built on existing third-party isolation tools. Do not
present cgroup limits as a security boundary and do not make a core sandbox a
shared prerequisite.

### 5.6 Journal versus semantic history

The journal is a bounded diagnostic byte stream and may start mid-ANSI or
mid-UTF-8. It cannot be the semantic execution history or replay source. A
future history feature needs a separate event/capture contract with
checkpoints.

### 5.7 Local IPC versus HTTP transports

The current architecture is Unix-socket, length-prefixed JSON. gRPC/SSE would
add a second transport and, for SSE, an HTTP server. Use the local socket for
the first implementation. A remote gateway can translate later if needed.

### 5.8 Same UID versus multi-tenant agents

Peer UID/PID checks cannot distinguish two agents running as the same user.
A capability token and a human/agent authority model are prerequisites for
multi-agent use. Even then, authorization is not OS isolation.

### 5.9 Human lock and its effect on agents

There is no authenticated human account or tenant-suspension object in the
current V0 implementation. `owner` is a logical identity string, and clients
are distinguished primarily by peer UID. Therefore "human account lock"
cannot be enforced consistently today.

If an identity/tenant layer is added, lock semantics must be explicit:

| Lock/revocation scope | Agent control | Agent observation | Running tasks |
|---|---|---|---|
| Desktop/screen lock | Unchanged | Unchanged | Continue; not a security boundary |
| Single capability-token revocation | Blocked for that token | Blocked for that token | Depends on revocation reason |
| Human tenant suspension | All descendant tokens blocked | All descendant tokens blocked | Depends on revocation reason |
| Session/Pane write lock | Blocked for that Session/Pane | Policy-dependent | Continue unless the lock is defined as a kill |
| Task/Session kill | N/A | N/A | Terminated by the lifecycle operation |

Tenant suspension must be **cascading** by default. If a human principal is
suspended, every token issued by that principal or tenant must become invalid;
requiring an operator to revoke each token individually leaves a window in
which a suspended account can still drive an agent.

Use a monotonic `revocation_epoch` rather than enumerating descendants: a
token carries the epoch at issue time, and authentication rejects it once the
principal's epoch advances. The parent/tenant relation is retained for audit
and ownership, not for cascade traversal. A single token can still be revoked
explicitly without suspending the whole tenant.

The default process action must depend on **why** the revocation happened:

| Revocation reason | Default channel action | Default process action |
|---|---|---|
| `administrative` | Revoke the selected token | Continue; durable work is preserved |
| `policy_violation` | Revoke the token or tenant | Freeze the affected process tree |
| `security_incident` | Revoke the tenant and all descendant tokens | Freeze by default; terminate only after explicit confirmation |
| `emergency_kill` | Revoke the tenant and all descendant tokens | Terminate the affected process tree |

Severity is itself an authorization decision:

- if severity is omitted, default to the conservative `freeze` behavior;
- only a high-trust principal, at least the human/root level, may mark a
  revocation as `administrative`;
- any authorized principal may escalate severity to `security`;
- a freeze must have a timeout and an escalation/alert policy, otherwise it
  becomes an unreviewed permanent resource hold.

`freeze` means stopping the process group, for example `SIGSTOP`, with an
explicit resume path. `terminate` means the normal graceful-stop policy,
followed by a forced kill after the grace period. A security suspension must
not depend on a human remembering to issue a second freeze command.

The key distinction is **authorization revocation versus process lifecycle**.
Current Lease expiry removes control authority but does not kill a Task.
Administrative revocation should follow the same rule. Security revocation is
different: it should stop work by default while preserving enough state for
investigation.

If "human account lock" means an OS-level screen lock or disabled login, QWE
cannot assume that `qwed` or an already-running agent stops. A user service
and same-UID processes may continue. Real isolation requires a separate UID,
namespace, or sandbox.

### 5.10 Scrollback and copy persistence

Direct copy of the visible screen does not require persistent scrollback. It
only needs the current composed pane contents and a pane-clipped selection
rectangle.

History copy is different. Workers currently keep 2000 lines of `vt100`
scrollback in memory. Detach/attach preserves it while the worker is alive;
worker restart or host reboot loses it. There is no persistent scrollback
file, mmap cache, or SQLite history today.

If HX-001 selects history rather than only the visible screen, the ADR must
choose:

- memory-only, tmux-like history;
- append-only bounded history on disk;
- an mmap/disk-backed history with explicit truncation and crash semantics.

Recommended split:

1. Copy scope includes the visible screen and the existing in-memory 2000-line
   scrollback.
2. Scrollback persistence remains memory-only for the copy feature and is
   deferred to AR-012. Do not introduce a third persisted representation
   beside the diagnostic journal and the future semantic execution log.

Do not silently treat the diagnostic journal as history. It may start
mid-ANSI or mid-UTF-8 and cannot reconstruct the pane. Persisting history also
needs retention, redaction, and disk-budget policy.

## 6. Proposed Sequencing

Stage 6.5 fixed the shared prerequisites. The remaining track order is:

| Order | Work | Why |
|---|---|---|
| 0 | Shared-prerequisite ADR: `ResourceScope`, `WorkerTopology`, `ChannelProfile` | Complete; accepted in ADR 0021 |
| 1 | Stage 7 remains resource governance | Do not mix cgroup hardening with agent channels |
| 2 | Session Layout is the default next feature track | Agent Execution may run in parallel only with an accepted ADR and sufficient staffing |
| 3 | Session Layout ADR: Pane model, compositor, copy and menu | HX-001/HX-003 need this; HX-002 follows |
| 4 | Agent Execution ADR: exec versus shell semantics, event schema, capability model | AR-001/002/004/007/009/015/020 |
| 5 | Agent output and history: non-TTY exec, summary, redaction, observer | AR-006/010/011/013 |
| 6 | Agent security: ownership, capability tokens, per-command credentials | AR-016/017/018 |
| 7 | External sandbox (separate ADR), floating overlays, and cross-pane orchestration | AR-014/019, separate ADR |

The shared-kernel correction closes the architecture-shape question. Stage 7
is next; Session Layout is the default feature track after it. A later
explicit capacity decision may move Agent Execution ahead, but the change must
record the corresponding deferral rather than imply parallel delivery.

If Agent Execution starts before the full compositor, it must adopt the
accepted `WorkerTopology` and `ChannelProfile` contracts. It may not build a
private bypass while waiting for those interfaces.

Do not start with a `run_command` RPC that injects a shell string into an
existing pane. That path combines quoting, command boundaries, timeout,
stdin behavior, and output attribution into one unreliable API.

## 7. Open Decisions

ADR 0032 resolves decisions 1-6 and 8-9 below.
[ADR 0026](adr/0026-fixed-floating-panes.md) resolves decision 10. The
remaining entries stay open for later shell-integration, core-dump, or
privileged-management work.

1. Is option A (real-time ancestor epoch checks) accepted for nested tenants?
2. Is the revocation reason/severity policy accepted, including fail-safe
   `freeze` and severity authorization?
3. Must AR-002 and AR-007 be delivered in the same ADR?
4. Does HX-001 copy include scrollback, and is persistence deferred to
   AR-012?
5. Does `exec` run a fresh process or a command inside a persistent shell?
6. Is PTY output always merged, with non-TTY exec as the only split-stream
   mode?
7. Which shells and platforms are in the first shell-integration matrix?
8. Is the capability token inherited by descriptor or delivered over a
   protected local socket?
9. Which target do `agent_stream` and `agent_observer` use: Session, Pane, or
   Task?
10. Is a floating program a Pane, a Window, or a new Overlay object?
11. Does a floating program survive detach, and what is its close policy?
12. Does core-dump enablement apply per Session, per Pane, or per command?
13. Which machine-management actions are read-only, and which require a
    separate privileged component?

## 8. Non-Goals

- No Docker/Podman execution backend in QWE core.
- No claim that cgroup limits are a security sandbox.
- No built-in sandbox implementation; use external third-party isolation.
- No gRPC/SSE/TCP listener in the first local implementation.
- No secret values in environment, argv, logs, SQLite, or agent context.
- No full tmux command-language or configuration compatibility.
- No full copy-mode clone as a prerequisite for basic selection.
- No cross-pane orchestration engine in the terminal runtime.
- No pgvector dependency in QWE core.
- No privileged host management exposed to agents without a separate ADR.

## 9. Validation Implications

When these features enter a milestone, add stable cases for:

- command start/end/exit code with a supported shell and a fallback shell;
- `idle` versus `done` versus `unknown`;
- PTY merged output versus non-TTY split output;
- slow subscriber backpressure and replay cursor;
- observer cannot write or resize;
- capability-token scope failures;
- revocation-epoch failure for an old token after tenant suspension;
- ancestor-epoch invalidation at each nested tenant level;
- unspecified revocation severity defaults to freeze;
- only a high-trust principal can mark a revocation as administrative;
- freeze timeout escalates or alerts instead of holding resources forever;
- secret redaction in agent output and persisted logs;
- copy selection clamped to the origin pane;
- copy release writes the system clipboard or returns an explicit error;
- copy of visible screen versus scrollback, including worker-restart behavior;
- floating overlay focus, z-order, drag, scroll routing, and close
  confirmation;
- menu keys never reach the PTY;
- core-dump toggle is opt-in, per-Session, and visibly reported.

Feature IDs in this document are provisional. Do not add them to
`docs/test-matrices.md` until the corresponding ADR and stage are accepted.

## 10. Roadmap Alignment

### 10.1 Stage numbering

The proposed Stage 6/7/8/9 numbering does not match the repository's current
stage numbers. Do not rewrite historical stage numbers: they are used by
experiments, branches, test evidence, release notes, and issue discussions.

| Proposed planning name | Repository equivalent | Current status |
|---|---|---|
| Stage 6: cgroup resource isolation/monitoring | Stage 7 / D6 Resource Governance | Complete |
| Stage 7: tmux operations | Stage 8 Session Layout | Complete |
| Stage 8: agent runtime features | Agent Execution Model | ADR 0032 accepted; first delivery complete |
| Stage 9: small UX features | no current milestone | Not accepted |

Use stable names in new documents, for example "Resource Governance",
"Session Layout", "Agent Execution Model", and "Terminal UX and Diagnostics".
If numbers are required, use a new mapping table rather than changing the
meaning of an existing Stage 6/7.

### 10.2 Stage-by-stage fit and conflicts

#### Resource Governance

Fit: good. The V0 per-Session worker, PTY, process tree, and cgroup v2 scope
are a useful base; ADR 0021 accepts the Pane-first target.

Conflicts and gaps:

- the current cgroup scope is per Session, while later panes, floating
  programs, and agent commands need finer attribution;
- the policy model is attached to a Pane record but enforced through a
  Session scope;
- a single Session scope cannot distinguish a base shell, a floating `htop`,
  and an agent exec task;
- qwed cannot currently locate a worker's scope without reading
  `/proc/<worker>/cgroup` or storing a scope reference;
- macOS has monitored semantics rather than hard cgroup enforcement.

Required extension:

- introduce a `ResourceScope` abstraction that maps Session, Pane, and Task to
  a backend without leaking cgroup details into the core object model;
- implement the accepted hierarchy with Pane-level enforcement in the first
  Stage 7 slice;
- add `ResourceBackend` implementations for Linux cgroup v2 and macOS
  monitoring;
- carry effective policy and usage back to qwed without making cgroup files
  the API.

Do not implement a second resource model for agent commands. Agent execution
should use the same resource-scope interface with a different target.

#### Session Layout / tmux operations

Fit: partial. Session, Pane, PTY, VT parser, attach, and scrollback already
exist, but the current assumptions are intentionally narrow.

Conflicts and gaps:

- V0 Design Baseline D053 assigns one worker and PTY to a Session; ADR 0021
  accepts one worker per Pane;
- `panes.session_id` is `UNIQUE`; there is no Window object, geometry,
  z-order, focus, or Pane kind;
- the Attach channel renders one stream and is exclusive; multi-client and
  observer work require a new channel policy;
- copy selection requires pane hit-testing and clipping;
- split/resize/focus require protocol and storage changes;
- the current client has no compositor, status bar, menu, or mouse-selection
  layer.

Required extension:

- implement the accepted one-worker-per-Pane topology during schema v11;
- add Pane geometry, `PaneKind=normal|floating`, z-index, and focus state;
- define a compositor contract and per-Pane VT state;
- keep copy, menu, and selection as client-side projections over a stable
  control protocol;
- keep observer as a separate subscription, not a weakened Attach mode.

This stage is the shared dependency for human multi-pane work and agent
multi-task execution. It should not be bypassed by an agent-only attach or
input path.

#### Agent Execution Model

Fit: partial. qwed authority, worker ownership, Lease, resource policy,
credentials, and idempotency are reusable. The agent-specific channels do not
exist yet.

Conflicts and gaps:

- no exec/stream/observer channel;
- no shell integration or command handle;
- no non-TTY exec mode;
- no capability/tenant model; same-UID clients are not distinguishable;
- no explicit signal/priority path;
- no bounded semantic event log;
- current journal is diagnostic and cannot be replayed;
- PTY output cannot split stdout/stderr;
- current credential delivery is per Session, not per command.

Required extension:

- implement the kernel + channel model from AR-020;
- define `exec` versus `shell_exec` before adding any `run_command` RPC;
- define the event schema, sequence/replay, backpressure, and observer
  contract;
- add principal/tenant/capability tokens with cascade revocation before
  multi-agent claims;
- keep semantic history separate from the journal;
- keep sandbox separate from cgroup resource policy.

This stage can run in parallel with Session Layout because the worker topology
and shared Pane/Task identity contract are now fixed; parallel work still
requires its own accepted Agent Execution ADR.

#### Terminal UX and Diagnostics

Fit: partial. Copy, menu, and diagnostics are client-side or control-plane
features. Floating programs are not a small polish item.

- copy and menu fit the Stage 8 compositor/TUI work;
- floating overlays require compositor geometry, z-order, focus, input
  routing, a second PTY, and a resource scope;
- core dumps are a security-sensitive lifecycle feature, not a menu-only
  feature;
- host management can expand QWE into a privileged administration daemon.

Required extension:

- ship copy and menu after the compositor exists;
- implement floating overlays in phases: fixed popup, draggable overlay,
  multiple overlays;
- treat core-dump enablement as an opt-in security policy with an ADR;
- keep machine management read-only unless a separate privileged component is
  designed.

### 10.3 Dependency graph

```text
Resource Governance
  -> ResourceScope / ResourceBackend
       -> Session Layout
            -> Worker topology
            -> Pane geometry / compositor
            -> Copy / menu / floating
       -> Agent Execution
            -> exec / stream / observer
            -> capability / tenant model
            -> semantic events / history

External sandbox integration (optional)
  -> agent isolation
  -> privileged machine management
  -> core-dump policy
```

The important cross-stage decisions are:

1. worker topology;
2. resource scope hierarchy;
3. exclusive Attach versus observer subscription;
4. same-UID capability model;
5. PTY versus non-TTY execution semantics.

### 10.4 Extension assessment

The current core is a good base, not a finished extension architecture.

Strong extension points:

- qwed as the single authority;
- one worker owning PTY/VT/process state;
- explicit Session/Pane/TaskInstance records;
- Lease and attachment authority;
- `ResourcePolicy` and capability reporting;
- credential broker with inherited descriptor;
- durable idempotency and reconciliation;
- bounded Attach and VT scrollback.

Weak points that should be abstracted before large features:

- resource policy is tied to a per-Session cgroup implementation;
- worker ownership is one-PTY-shaped;
- Attach is one-stream-shaped and exclusive;
- there is no principal/capability object;
- there is no exec/event channel;
- there is no compositor or Pane geometry;
- journal is not a replay/history format;
- sandbox is external and must not be conflated with resource limits.

Recommended abstractions:

| Abstraction | Purpose |
|---|---|
| `ResourceScope` / `ResourceBackend` | Session/Pane/Task policy without cgroup leakage |
| `WorkerTopology` | One worker per Pane; a Session groups Pane workers |
| `ChannelProfile` | Human interactive versus agent programmatic projection, with an opaque `AuthContext` |
| `Principal` / `CapabilityToken` / `RevocationEpoch` | Human-root and agent-tenant authorization |
| `ExecMode` | PTY merged stream versus non-TTY split pipes |
| `PaneGeometry` / `Compositor` | Split, focus, copy, floating overlays |
| `EventStream` / `OutputProjection` | Raw, semantic, summary, and redacted views |
| External sandbox adapter (optional) | Third-party filesystem/network/UID isolation; not a shared prerequisite |

### 10.5 Recommendation

The proposed ordering is directionally reasonable:

```text
Resource Governance
  -> Session Layout
  -> Agent Execution Model
  -> Terminal UX and Diagnostics
```

with these corrections:

- do not renumber existing Stage 6/7 history;
- implement the accepted resource-scope hierarchy before Session Layout;
- implement the accepted one-worker-per-Pane topology before panes or floating
  overlays;
- preserve the accepted opaque `AuthContext` in `ChannelProfile` while the
  capability ADR is completed;
- keep copy/menu with the compositor stage;
- move floating overlays out of "small features";
- make core-dump enablement a security ADR, not a menu checkbox;
- do not put a core sandbox implementation in the shared prerequisites;
- do not let agent work create a second state path around Attach, Pane, or
  resource scope.

If agent execution is the higher product priority, the minimum prerequisite
is still the shared Pane/Task identity and worker-topology decision. The agent
channel can then start before the full human compositor, but it must use the
same kernel and state.

The Stage 6.4 spike, Stage 6.5 decision gate, Resource Governance, and Session
Layout are complete. Agent Execution Model now has an accepted
[ADR 0032](adr/0032-agent-execution-model.md); implementation may start using
the shared Pane, Task, ChannelProfile, and ResourceScope contracts.

## 11. Relationship to Existing Documents

- Shared prerequisites are accepted in
  [ADR 0021](adr/0021-shared-prerequisites.md).
- This document does not supersede `docs/design-baseline.md` or any accepted
  ADR.
- Stage 7 remains defined by `docs/stage7-resource-governance.md`.
- The Stage 8 object-model proposal is not accepted by this document; it is a
  separate decision.
- E05 Agent Runtime remains the correct long-term home for agent-specific
  execution semantics, credentials, and task recovery; sandboxing is an
  external integration, not a core requirement.
- Any accepted item must update the development plan, roadmap, test matrices,
  and relevant ADR in the same change.
