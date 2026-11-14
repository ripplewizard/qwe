# ADR 0026: Fixed Floating Panes

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0024](0024-pane-composition-controls.md)
Implementation state: Multiple fixed floating Panes, z-order rendering, focus,
input, and close are implemented. Dragging and mouse hit-testing remain open.

## Context

Stage 8 needs an overlay phase before free dragging. A floating program still
requires one worker, one PTY, one VT screen, and one ResourceScope. It must not
be implemented as a special rendering mode inside a tiled Pane.

## Decision

### 1. Floating Pane identity

`PANE_CREATE` accepts `pane_kind=floating`. A floating Pane:

- owns one worker, PTY, journal, VT state, Task, and ResourceScope;
- persists `kind=floating` and an increasing Session-local z-index;
- does not participate in tiled layout order or `PANE_REORDER`;
- cannot use `after_pane_id`.

Normal tiled geometry ignores floating Panes. Floating Panes render afterward
in ascending z-index, with layout order and Pane ID as deterministic tie
breakers.

### 2. Fixed popup geometry

The current compositor derives a centered popup from the available Pane
viewport:

- 80% of available columns;
- 75% of available rows;
- integer centering with a minimum one-cell size.

Geometry is recomputed on terminal resize and is not a freeform persisted
rectangle. A later drag ADR must add durable bounds and input hit-testing.

### 3. Focus and lifecycle

Floating Panes participate in normal focus cycling and Pane-targeted Attach
input. `PANE_FOCUS`, close, process-exit handling, and ResourceScope teardown
use the same paths as tiled Panes.

`Ctrl-A f` creates and opens a floating shell. The quick menu exposes the same
action. Closing it returns focus to an available Pane; it never closes the last
visible Pane.

## Non-Goals

- Dragging, resizing by mouse, or arbitrary overlay bounds.
- Click-to-focus or z-order mutation from mouse input.
- More than one Pane behind one floating worker.
- Agent-specific overlays or host-machine windows.

## Validation

LAYOUT-011 creates a floating shell over a two-Pane tiled Session, verifies the
overlay title/focus, routes raw input to it, and closes it through the normal
Pane teardown path.

## References

- [ADR 0021](0021-shared-prerequisites.md)
- [ADR 0024](0024-pane-composition-controls.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
