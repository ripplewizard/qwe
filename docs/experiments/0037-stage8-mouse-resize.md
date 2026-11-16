# Experiment 0037: Stage 8 Mouse Resize Handle

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0029](../adr/0029-mouse-resize-handle.md)

## Scope

Validate deterministic SGR resize-handle interaction on a floating Pane.

## Observations

- The floating bottom-right cell renders as a visible `+` handle.
- SGR press/motion/release on that cell updates a local width/height draft.
- Release persists one rectangle and reports a distinct resize status.
- `SESSION_LAYOUT` returns the resized dimensions.
- The draft is clamped to the usable viewport.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-014.

## Limitations

- Mouse z-order changes, resize guides, and aspect-ratio constraints are not
  implemented.
- Mouse events inside content continue to depend on the child VT mouse mode.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
