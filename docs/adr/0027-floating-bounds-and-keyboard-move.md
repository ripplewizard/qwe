# ADR 0027: Durable Floating Bounds and Keyboard Move

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0026](0026-fixed-floating-panes.md)
Implementation state: Schema v13, durable floating bounds, and keyboard
move/resize are implemented. Mouse drag and hit-testing remain open.

## Context

Fixed centered popups prove worker-backed floating Panes, but users need stable
overlay placement. Mouse dragging requires hit-testing, child mouse-event
routing, and focus/z-order policy. Explicit bounds and an authorized mutation
path should exist first.

## Decision

### 1. Persisted bounds

Schema v13 adds nullable `floating_x`, `floating_y`, `floating_rows`, and
`floating_columns`.

- A floating Pane may have no saved bounds and use the centered popup default.
- Saved bounds are positive and clamped to the usable viewport at render time.
- Tiled Panes cannot store floating bounds.
- Bounds survive qwed restart and preserve Pane identity.

### 2. Authorized mutation

`PANE_FLOATING_UPDATE` accepts `pane_id` plus a four-field `rect`. It requires
the target floating Pane's LayoutMutation authority, or an unattached target
with a current Session Lease. Tiled targets are rejected.

### 3. Keyboard move mode

`Ctrl-A d` enters floating move mode when the focused Pane is floating:

- `h/j/k/l` move the popup by one cell;
- `H/L` shrink/grow width;
- `K/J` shrink/grow height;
- `y` or Enter saves;
- `q` or Escape cancels.

The draft is clamped to the usable viewport. Resize, process exit, or losing
the draft cancels cleanly. The compositor sends one durable update on save.

## Non-Goals

- Mouse dragging, resize handles, or click-to-focus.
- Persisting z-order changes from mouse interaction.
- Child-program mouse-event delivery through overlay hit-testing.
- Arbitrary sub-cell or pixel geometry.

## Validation

LAYOUT-012 opens a floating Pane, moves and resizes it in keyboard mode, saves
the bounds, and verifies `SESSION_LAYOUT` returns the persisted rectangle.

## References

- [ADR 0023](0023-durable-pane-layout.md)
- [ADR 0026](0026-fixed-floating-panes.md)
- [Storage Migration](../storage-migration.md)
