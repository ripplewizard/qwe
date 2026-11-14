# ADR 0030: Floating Pane Click-to-Raise

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0028](0028-mouse-drag-and-hit-testing.md)
Implementation state: Floating click-to-raise and persisted z-order are
implemented.

## Context

Topmost-z hit-testing selects the visible floating Pane, but a lower exposed
overlay cannot become topmost without a z-order mutation. Click-to-raise is the
minimal expected window-manager behavior and can reuse existing LayoutMutation
authority.

## Decision

`PANE_FLOATING_RAISE` raises one floating Pane to the next Session-local
z-index. It rejects tiled Panes and requires the target Pane's LayoutMutation
authority or a current Session Lease for an unattached target.

On left mouse press over any floating Pane:

1. hit-test the rendered stack;
2. raise the selected floating Pane;
3. reorder client views and Attach targets by persisted z-index;
4. focus the raised Pane;
5. continue title drag, corner resize, or content routing.

Right-button and wheel events do not change z-order.

## Validation

LAYOUT-015 creates two overlapping floating Panes, clicks an exposed region of
the lower overlay, and verifies the persisted floating order and raised
z-index.

## References

- [ADR 0026](0026-fixed-floating-panes.md)
- [ADR 0028](0028-mouse-drag-and-hit-testing.md)
- [ADR 0029](0029-mouse-resize-handle.md)
