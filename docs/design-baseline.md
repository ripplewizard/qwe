# QWE Design Baseline

Status: Active Development Baseline
Updated: 2026-11-16

## 1. Purpose

This is the current implementation baseline. Revisions are tracked by Git
history rather than versioned filenames.

## 2. Authority Model

| ID | Decision |
|---|---|
| D036 | `qwed` is the sole authority for SQLite Session, Task, and Lease state. |
| D037 | One `qwed` supervisor is authoritative for the public API, SQLite, worker inventory, and control state. Worker inventory keys by Pane. |
| D038 | Startup reconciliation completes before clients are accepted; runtime worker loss is event-first with a maximum 2 second safety scan. |
| D039 | Worker rediscovery validates socket path, worker PID/start token, Session ID, and Pane ID. |
| D040 | A missing or identity-mismatched worker becomes `lost`; a live but unresponsive worker is `unreachable` and is not declared exited. |
| D053 | The runtime runs one worker per Pane; a Session is a logical grouping of Pane workers. |
| D054 | qwed addresses persisted worker identity, runtime discovery, health, recovery, and cleanup by Pane. |
| D055 | ResourceScope targets Session, Pane, or Task; Linux enforcement starts at Pane scope with Session as the logical parent. |
| D056 | ChannelProfile carries an opaque AuthContext and a projection/capability policy; human observer is a qwed-owned read-only projection. |
| D057 | Principal is a first-class identity anchor with `parent_id`, monotonic `epoch`, and tombstone `revoked_at`. |
| D058 | ResourceScope teardown must cover all descendants, including detached process groups, before the scope is tombstoned. |
| D059 | Sandboxing is an external third-party integration and is not a QWE core prerequisite. |
| D060 | Public interactive Attach may select an optional Pane; omission preserves default-Pane compatibility. |
| D061 | Each displayed Pane keeps one exclusive interactive writer; a compositor may hold one channel per Pane. |
| D062 | qwed returns viewport-relative layout geometry from shared deterministic tiling; Slice 1 does not persist geometry. |
| D063 | The client compositor is a projection and never becomes authority for inventory, attachment, process, or terminal state. |
| D064 | Schema v12 persists Pane kind, order, relative weight, z-index, and Session focus. |
| D065 | Focus is a user preference; `PANE_FOCUS` requires attachment authority on the target Pane. |
| D066 | `PANE_CREATE.after_pane_id` inserts relative to live layout order; `PANE_REORDER` replaces the complete live order transactionally. |
| D067 | Closed and lost worker records remain history but are excluded from active composition and reorder membership. |
| D068 | Copy selection is client-local, clamped to the focused Pane, and limited to the current visible screen in Stage 8.4. |
| D069 | Clipboard writes return an explicit provider/result; menu bytes are consumed before PTY input. |
| D070 | A floating Pane is a full Pane worker/PTY/ResourceScope and persists `kind=floating` plus z-index. |
| D071 | The current floating phase uses deterministic centered popup geometry and renders fixed overlays in z-order; dragging is not yet enabled. |
| D072 | Schema v13 persists optional floating rectangles; render clamps them to the usable viewport. |
| D073 | `PANE_FLOATING_UPDATE` and `Ctrl-A d` provide authorized keyboard move/resize; D074-D075 add mouse dragging and hit-testing. |
| D074 | Compositor mouse input uses SGR button-event tracking and topmost-z Pane hit-testing. |
| D075 | Floating title drags update one draft rectangle and persist once on left-button release; content mouse events are forwarded only when the child mouse protocol is enabled. |
| D076 | Floating bottom-right mouse resize uses the same draft/persist boundary; D077 adds mouse z-order changes. |
| D077 | Left mouse press on a floating Pane raises its persisted z-index before focus, drag, resize, or content routing. |
| D078 | Copy mode supports bounded client VT scrollback and focused-Pane mouse selection; state replacement invalidates selection. |
| D079 | The first agent command primitive is fresh-process `exec`; `shell_exec` is deferred until separately specified. |
| D080 | Every agent command has a stable command handle; `wait_idle` ships in the same delivery and distinguishes idle, done, failed, lost, and unknown. |
| D081 | Agent execution reuses Pane workers and ResourceScopes; PTY output is merged while non-PTY exec exposes separate stdout/stderr event streams. |
| D082 | Agent capabilities are scoped and checked against real-time ancestor revocation epochs; unspecified revocation severity fails safe to freeze. |
| D083 | Agent event history is a bounded semantic log separate from the diagnostic journal; replay gaps and lost recovery states are explicit. |
| D084 | The first Linux sandbox backend is external Bubblewrap; QWE profiles stay fail closed and Pane cgroups remain the sole resource authority. |

ADRs [0011](adr/0011-consolidate-control-plane.md) through
[0015](adr/0015-worker-liveness-reconciliation.md) are accepted. qwed is the
single supervisor and directly owns persistent worker control and health.

The terminal channel, object model, authorization matrix, and worker-health
contract are defined by those ADRs and represented in the
[canonical protocol schema](../protocol/schema/qwe-v1.schema.json). The current
client and worker operations are documented in [IPC Protocol](ipc-protocol.md).

## 3. Lease Semantics

| ID | Decision |
|---|---|
| D041 | Lease deadlines use UTC wall-clock milliseconds. |
| D042 | Lease owner is a logical client identity, not a PID or socket instance. |
| D043 | Expiry does not terminate Tasks or Sessions; it only removes control authority. |
| D044 | Claim/takeover requires an expired or released lease and increments generation. |
| D045 | Release writes a generation tombstone rather than deleting history. |

## 4. Output and Backpressure

| ID | Decision |
|---|---|
| D046 | The journal is per Pane worker, not per client or Session. |
| D047 | Each Attach gets a private 1 MiB journal tail and, after the Stage 2 cutover, an independent 1 MiB live-output queue. |
| D048 | Journal tail is diagnostic-only and never a replay format; screen state is authoritative. |
| D049 | Compact structured VT state with cursor and terminal modes is a D1 requirement. |

## 5. Required Product Capabilities

These capabilities are explicitly tracked rather than hidden under generic
deferred scope:

| ID | Capability | Milestone |
|---|---|---|
| D050 | API Key Manager: secrets never in argv, environment, workspace files, or logs | D3 |
| D051 | ResourceScope-backed CPU, memory, and process limits for Session, Pane, and Task targets; first enforcement is Pane-level | D3 + Stage 7 |
| D052 | Forced teardown: `kill-session`, `kill-server`, and batch Task stop | D3 |

API Key injection must use an explicit broker or inherited file descriptor.
Secrets must be removed from the child environment before exec.

Stage 4 implementation uses a private credential source, zeroizing qwed
memory, a one-shot broker socket, and final-child descriptor 3. CPU resource
policy is tree-scoped. V0 Linux uses per-Session cgroup v2 scopes for hard
memory, task-count, and CPU controls; the accepted Stage 7 target moves hard
scope ownership to the Pane under [ADR 0021](adr/0021-shared-prerequisites.md).
macOS uses libproc monitoring with explicit bounded-overshoot semantics. Limits
can be changed while a Session is running under
[ADR 0018](adr/0018-pane-resource-enforcement.md). Forced Session, batch Task,
and server teardown are implemented under
[ADR 0017](adr/0017-stage-4-service-security-and-capabilities.md).

The journal may start mid-ANSI or mid-UTF-8 sequence. No feature may parse the
journal to reconstruct terminal state. Recording or diff features require a
separate gap-aware framing contract with VT state checkpoints.

## 6. Security

- Unix sockets are mode `0600`.
- Peer UID equals the server UID.
- Peer checks remain defense in depth even though owner-only sockets already
  reject other UIDs at the OS layer.
- Cross-UID, stale epoch, malformed frame, oversized input, and idempotency
  tests are required in D3.
- Pane-level attachment authority is implemented. Session Leases remain the
  fence for Session-wide lifecycle control.
- Channel profiles share the same authoritative Session/Pane/Task state; an
  observer cannot acquire input or control authority.

## 7. Recovery Matrix

| Failure | Result |
|---|---|
| `qwed` SIGKILL | Workers continue; supervisor restart reconciles worker inventory |
| Worker SIGKILL | The affected Pane worker records `lost`; unrelated Panes remain healthy |
| Worker process alive but unresponsive | Health becomes `unreachable` for that Pane; Session/Task are not falsely marked exited |
| Supervisor unavailable | Clients receive an explicit transport error |
| OS reboot | Out of V0 scope |

Worker exits are observed through `SIGCHLD`/`waitpid`; control-channel EOF marks
health unreachable immediately, and a two-second safety scan covers missed
events.

## 8. Development Gates

- Automated POC evidence exists on Linux and macOS.
- Manual POC visual sign-off remains a non-blocking audit task.
- Stage 0 design acceptance, stable test IDs, protocol-schema drift checking,
  macOS platform spikes, and VT capability confirmation are complete.
- D1 adds Resize and complete VT state.
- D2 adds durable idempotency and Task control.
- D3 adds Key Manager, Pane limits, teardown, services, and security negatives.
- D4 performs release-mode soak and user-facing acceptance. Stage 5 also adds
  checksummed native packages, isolated installation and upgrade recovery,
  strict macOS Developer ID/notarization gates, and an explicit supported
  platform matrix; public trust is not claimed before those gates pass.
  ADR 0019 temporarily marks macOS binary distribution as
  `local-self-signed`/non-public until a Developer ID identity is available.
  ADR 0020 scopes V0 to Linux-first release and defers public macOS to a
  separate post-V0 milestone. D4/Stage 5 is complete for Linux-first V0.
- Stage 6.4 recorded disposable multi-Pane evidence. Stage 6.5 accepted
  [ADR 0021](adr/0021-shared-prerequisites.md) and the v10-to-v11 invariants in
  [Storage Migration](storage-migration.md). Stage 7 implemented production
  multi-Pane inventory, control, resources, and recovery.
- Stage 8 slice 1 implements [ADR 0022](adr/0022-session-layout-compositor.md):
  Pane-targeted Attach, `SESSION_LAYOUT`, shared tiling, and
  `qwe session compose`. Slice 2 adds schema v12 and durable focus under
  [ADR 0023](adr/0023-durable-pane-layout.md). Slice 3 adds relative split,
  close, and reorder under [ADR 0024](adr/0024-pane-composition-controls.md).
  Slice 4 adds focused-Pane copy and the quick menu under
  [ADR 0025](adr/0025-copy-and-quick-menu.md). Slice 5 adds fixed floating
  Panes under [ADR 0026](adr/0026-fixed-floating-panes.md). Slice 6 adds
  durable floating bounds and keyboard move/resize under
  [ADR 0027](adr/0027-floating-bounds-and-keyboard-move.md). Slice 7 adds mouse
  title drag and hit-testing under
  [ADR 0028](adr/0028-mouse-drag-and-hit-testing.md). Slice 8 adds corner mouse
  resize under [ADR 0029](adr/0029-mouse-resize-handle.md). Slice 9 adds
  click-to-raise z-order under [ADR 0030](adr/0030-floating-z-order.md).
  Slice 10 adds scrollback and mouse copy under
  [ADR 0031](adr/0031-scrollback-and-mouse-copy.md). Stage 8 implementation
  and supported-platform qualification are complete under
  [Session Layout](stage8-session-layout.md). Agent Execution Model is
  accepted under [ADR 0032](adr/0032-agent-execution-model.md); the accepted
  first delivery is complete through Stage 9.5. Boundaries and deferred
  follow-ons are tracked in
  [Agent Execution Model](agent-execution-model.md).
  The Linux Bubblewrap `AGENT_EXEC` and sandboxed-Pane implementation is
  complete for the current x86_64 product slice under
  [ADR 0033](adr/0033-bubblewrap-sandbox-adapter.md) and
  [ADR 0034](adr/0034-sandboxed-pane-and-pty.md); distribution evidence and
  deferred targets are tracked in
  [Experiment 0051](experiments/0051-sandbox-pane-poc-gates.md).
