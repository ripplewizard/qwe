# ADR 0029: Mouse Resize Handle

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0028](0028-mouse-drag-and-hit-testing.md)
Implementation state: Floating bottom-right mouse resize is implemented.

## Context

ADR 0028 deliberately limited mouse interaction to title-row dragging. Floating
bounds already support keyboard width/height changes, so the same draft/persist
path can support a minimal pointer resize handle.

## Decision

Each floating Pane renders a visible bottom-right resize marker. The
bottom-right cell is a hit target.

Left press on that cell starts a resize draft. Motion adjusts width and height
from the original rectangle using terminal-cell deltas. The draft is clamped to
the usable viewport. Release sends one `PANE_FLOATING_UPDATE`; no resize event
is forwarded to the child PTY.

Title-row drag and content mouse routing retain the ADR 0028 behavior. Mouse
z-order changes remain outside this decision.

## Validation

LAYOUT-014 injects SGR press/motion/release into a real compositor PTY,
resizes the bottom-right handle, and verifies the persisted rectangle.

## References

- [ADR 0027](0027-floating-bounds-and-keyboard-move.md)
- [ADR 0028](0028-mouse-drag-and-hit-testing.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
