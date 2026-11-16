# Experiment 0032: Stage 8 Composition Controls

Date: 2026-11-10
Status: Pass on Linux x86_64
Related: [ADR 0024](../adr/0024-pane-composition-controls.md)

## Scope

Validate relative split, close, focus-preserving reorder, and active-layout
membership through real qwed, Pane workers, PTYs, and the interactive
compositor.

## Observations

- `Ctrl-A c` created a shell Pane immediately after focus, attached it, resized
  all Panes, and routed input to the new Pane.
- `Ctrl-A x` closed the new Pane and retained the two surviving channels.
- Closed worker history no longer appeared as an active layout member.
- `Ctrl-A h` persisted a focus-preserving reorder.
- qwed restart retained the reordered live set and focused Pane.
- The last visible Pane remains protected from compositor-level close.

## Limitations

- Split ratios remain metadata-driven; mouse dragging and arbitrary ratio UI
  are not implemented.
- Lost-Pane diagnostics and too-small-viewport presentation remain future
  work.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
