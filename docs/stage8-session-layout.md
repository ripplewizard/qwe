# Stage 8: Session Layout

Status: Complete; implementation and supported-platform qualification pass
Date: 2026-11-14
Decisions: [ADR 0022](adr/0022-session-layout-compositor.md),
[ADR 0023](adr/0023-durable-pane-layout.md),
[ADR 0024](adr/0024-pane-composition-controls.md),
[ADR 0025](adr/0025-copy-and-quick-menu.md),
[ADR 0026](adr/0026-fixed-floating-panes.md),
[ADR 0027](adr/0027-floating-bounds-and-keyboard-move.md),
[ADR 0028](adr/0028-mouse-drag-and-hit-testing.md),
[ADR 0029](adr/0029-mouse-resize-handle.md),
[ADR 0030](adr/0030-floating-z-order.md),
[ADR 0031](adr/0031-scrollback-and-mouse-copy.md)

## Product Definition

Stage 8 makes a multi-Pane Session usable as one terminal surface while
preserving Pane-level worker, PTY, resource, and attachment ownership.

The product target is:

> QWE presents durable terminal Panes through a compositor without turning the
> client into a second runtime authority.

## Slice 1

Implemented:

- Optional `pane_id` on Attach Open, accepted through qwed and validated by the
  selected worker.
- Pane-scoped attachment authority for interactive channels.
- `SESSION_LAYOUT` returning a deterministic viewport-relative grid.
- Shared `qwe-core` tiling used by qwed and the client.
- `qwe session layout` for protocol inspection.
- `qwe session compose` with live VT rendering, focus routing, resize, process
  exit display, and clean detach.
- Concurrent exclusive Attach channels to distinct Panes in one Session.
- `just stage8-layout` covering LAYOUT-001 through LAYOUT-017.

Slice 2 adds schema v12, persisted Pane kind, order, weight, z-index, and focus.
`PANE_FOCUS` keeps focus restart-stable, and default Attach resolves persisted
focus before layout order. Slice 3 adds relative split, close, and reorder in
the API and `Ctrl-A c/x/h/l` compositor controls.

Slice 4 adds focused-Pane visible-screen copy, explicit clipboard outcomes, and
the `Ctrl-A m` quick menu. Floating overlays, scrollback copy, arbitrary split
ratios, and mouse dragging remain open.

Slice 5 adds fixed floating Panes with independent workers, centered popup
geometry, persisted z-order, focus/input routing, and normal close semantics.
Dragging, mouse hit-testing, scrollback copy, and arbitrary split ratios remain
open.

Slice 6 adds schema v13, durable floating rectangles, and `Ctrl-A d` keyboard
move/resize. Mouse dragging, hit-testing, scrollback copy, and arbitrary split
ratios remain open.

Slice 7 adds SGR mouse parsing, topmost-z hit-testing, floating title dragging,
and child mouse-event forwarding. Scrollback copy and arbitrary split ratios
remain open.

Slice 8 adds a bottom-right floating mouse resize handle with draft clamping
and one persisted update on release. Scrollback copy, arbitrary split ratios,
and mouse z-order changes remain open.

Slice 9 adds floating click-to-raise with persisted z-order and view/target
reordering. Scrollback copy and arbitrary split ratios remain open.

Slice 10 adds client scrollback navigation and focused-Pane mouse selection
under [ADR 0031](adr/0031-scrollback-and-mouse-copy.md). Native terminal
selection and arbitrary split ratios remain outside the current implementation.

## Workstreams

### L1. Routing and Compositor Foundation

Status: Complete for slice 1.

- Add Pane-targeted Attach without changing default-Pane compatibility.
- Share one deterministic tiling function between qwed and the client.
- Render every attached Pane from its own VT state.
- Route raw input only to the focused Pane.
- Resize every Pane independently when the viewport changes.

### L2. Durable Layout Metadata

Status: Complete for metadata and focus.

- Persist Pane kind, layout order, z-index, and split ratios or an equivalent
  layout tree.
- Persist focused Pane as a user preference, not attachment authority.
- Migrate schema v11 data with one normal Pane per existing Session.
- Preserve stable Pane identities across restart.

Implemented with per-Pane order and relative weight metadata plus a Session
focus record. v11 migration assigns stable sequential order and equal weight;
authorized split/reorder operations remain in L3.

### L3. Composition Controls

Status: Split, close, focus, and reorder complete; visual edge cases open.

- Split, close, focus, and reorder actions through authorized control requests.
- Keep compositor shortcuts client-side while authorization remains in qwed.
- Define minimum-size and too-small-viewport behavior.
- Make exited and lost Panes visibly distinct without moving live state into
  qwed.

Implemented `PANE_CREATE.after_pane_id`, authorized `PANE_REORDER`, dynamic
compositor attach/remove, and focus-preserving left/right moves. Closed and lost
worker records are excluded from active layout membership. Minimum-size
diagnostics and lost-Pane presentation remain open.

### L4. Copy and Menu

Status: Complete for visible-screen/scrollback copy, mouse selection, and keyboard menu.

- Clamp selection to the originating Pane.
- Define visible-screen versus scrollback behavior across worker restart.
- Return explicit success or failure for clipboard writes.
- Keep menu keys out of the PTY.

Implemented under [ADR 0025](adr/0025-copy-and-quick-menu.md). Selection is
clamped to the focused Pane's visible screen. Snapshot replacement, resize,
process exit, and Attach close cancel selection. Scrollback navigation and mouse
selection are implemented under
[ADR 0031](adr/0031-scrollback-and-mouse-copy.md).

### L5. Floating Overlays

Status: Complete for fixed/draggable floating Panes, z-order, mouse drag/resize, and click-to-raise.

- Implement fixed popup first, then drag, then multiple overlays.
- Give each floating program its own Pane worker, PTY, and ResourceScope.
- Define z-order and focus transitions before enabling free dragging.

Implemented as real Pane workers under
[ADR 0026](adr/0026-fixed-floating-panes.md). `Ctrl-A f` opens a centered
floating shell; z-order is durable and tiled reorder excludes floating Panes.
[ADR 0027](adr/0027-floating-bounds-and-keyboard-move.md) adds durable bounds
and `Ctrl-A d` move/resize. [ADR 0028](adr/0028-mouse-drag-and-hit-testing.md)
adds SGR mouse title dragging, topmost-z hit-testing, click focus, and child
mouse-event routing. [ADR 0029](adr/0029-mouse-resize-handle.md) adds
bottom-right mouse resize. [ADR 0030](adr/0030-floating-z-order.md) adds
click-to-raise and persisted view reordering.

## Exit Criteria

Stage 8 is complete only when:

- Pane-targeted Attach and all LAYOUT rows pass on supported platforms;
- layout metadata survives qwed restart and preserves Pane identity;
- split/close/focus controls pass authorization and recovery tests;
- copy selection is Pane-clipped and clipboard failure is explicit;
- menu keys never reach PTY input;
- fixed and draggable floating Panes pass resource-scope teardown;
- protocol, storage, CLI, architecture, and test-matrix docs remain aligned.

## Non-Goals

- A second daemon, a second PTY owner, or agent-specific terminal state.
- Multi-writer Attach to one Pane.
- Filesystem, network, UID, or namespace sandboxing.
- Persistent terminal capture or semantic command history.
- Host or machine management.
