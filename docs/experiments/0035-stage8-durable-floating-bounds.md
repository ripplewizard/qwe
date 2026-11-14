# Experiment 0035: Stage 8 Durable Floating Bounds

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0027](../adr/0027-floating-bounds-and-keyboard-move.md)

## Scope

Validate schema v13 migration, authorized floating rectangle updates, and
keyboard move/resize without changing Pane/worker identity.

## Observations

- `Ctrl-A d` entered move mode only on a focused floating Pane.
- `h/j/k/l` moved the draft popup and `H/L`/`K/J` changed its dimensions.
- Saving issued one authorized `PANE_FLOATING_UPDATE`.
- `SESSION_LAYOUT` returned the persisted rectangle while the Pane remained
  live.
- Clamping keeps saved bounds inside the usable viewport.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-012.

## Limitations

- Mouse dragging, resize handles, click-to-focus, and child mouse-event routing
  are not implemented.
- Bounds use terminal cells, not pixels.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
