# Experiment 0034: Stage 8 Fixed Floating Panes

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0026](../adr/0026-fixed-floating-panes.md)

## Scope

Validate a fixed floating Pane as a full worker-backed terminal overlay rather
than a special rendering mode inside a tiled Pane.

## Observations

- `Ctrl-A f` created a floating shell with an independent worker and PTY.
- The compositor rendered a centered popup after the tiled Panes using
  persisted z-index ordering.
- Focus, raw input, resize, and close used the normal Pane-targeted paths.
- Closing the floating Pane preserved the two tiled survivors.
- Tiled reorder excluded the floating Pane.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-011.

## Limitations

- Floating bounds are derived from viewport proportions, not persisted freeform
  geometry.
- Dragging, resize handles, click-to-focus, and mouse z-order changes are not
  implemented.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
