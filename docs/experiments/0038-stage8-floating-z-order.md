# Experiment 0038: Stage 8 Floating Z-Order

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0030](../adr/0030-floating-z-order.md)

## Scope

Validate click-to-raise with overlapping floating Panes and persistence of the
new z-order.

## Observations

- Two floating Panes rendered in persisted z-order.
- A click on an exposed part of the lower overlay hit that Pane rather than the
  visually higher one.
- `PANE_FLOATING_RAISE` assigned the next z-index.
- The compositor reordered views and Attach targets and focused the raised
  Pane.
- `SESSION_LAYOUT` returned reversed floating z-order after the click.
- Closing both overlays preserved the tiled Session.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-015.

## Limitations

- Right-button and wheel events do not alter z-order.
- There is no taskbar, overlay list, or keyboard z-order command.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
