# Experiment 0036: Stage 8 Mouse Drag and Hit-Testing

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0028](../adr/0028-mouse-drag-and-hit-testing.md)

## Scope

Validate deterministic mouse drag through real PTY input and verify topmost-z
hit-testing and persisted floating bounds.

## Observations

- Compositor startup enabled `1002` button-event tracking and `1006` SGR
  encoding.
- SGR press/motion/release bytes were parsed incrementally.
- The title-row hit selected the top floating Pane and updated a local draft.
- Release issued one authorized floating-bounds update.
- `SESSION_LAYOUT` returned the dragged rectangle.
- Content events are forwarded only when the target Pane reports an enabled VT
  mouse protocol.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-013.

## Limitations

- Mouse resize handles and mouse-driven z-order changes are not implemented.
- The compositor does not maintain hover state beyond an active press/drag.
- Local terminal text selection is unavailable while compositor mouse tracking
  is active.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
