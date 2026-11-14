# ADR 0028: Mouse Drag and Pane Hit-Testing

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0027](0027-floating-bounds-and-keyboard-move.md)
Implementation state: SGR left-button drag, topmost Pane hit-testing, and
content mouse forwarding are implemented.

## Context

Keyboard move makes floating bounds durable, but users need direct pointer
interaction. The compositor can simulate and receive terminal mouse events via
SGR, but it must distinguish title-bar dragging from child-program mouse input.

## Decision

### 1. Mouse encoding

While composing, the client enables button-event tracking and SGR encoding:

```text
CSI ? 1002 h
CSI ? 1006 h
```

The existing terminal reset disables mouse modes on detach. The client parses
SGR press, motion, and release sequences incrementally, including sequences
split across reads.

### 2. Hit testing

Mouse coordinates are converted from one-based terminal cells to zero-based
compositor coordinates. Pane hit-testing walks the rendered view stack in
reverse, so the highest z-index floating Pane wins.

Left press on a floating Pane's title row starts a drag. Motion updates only a
client-local draft rectangle, clamped to the usable viewport. Release sends one
`PANE_FLOATING_UPDATE` and refreshes the persisted bounds.

### 3. Child mouse routing

Non-title content clicks focus the hit Pane on left press. If that Pane's VT
screen has a mouse protocol enabled, press/motion/release bytes are forwarded
unchanged through its Pane-targeted Attach input channel. Right-button and wheel
events follow the same content-routing path.

Title-row drag events are never forwarded to the child PTY.

## Non-Goals

- Local terminal text selection while compositor mouse tracking is active.
- Resize handles or z-order changes by mouse.
- Pixel geometry, sub-cell dragging, or drag inertia.
- Forwarding a mouse event to a Pane that has no mouse protocol enabled.

## Validation

LAYOUT-013 injects SGR press/motion/release bytes into a real compositor PTY,
drags a floating title row, and verifies the persisted rectangle. Parser unit
tests cover complete and split sequences.

## References

- [ADR 0025](0025-copy-and-quick-menu.md)
- [ADR 0027](0027-floating-bounds-and-keyboard-move.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
