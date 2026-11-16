# Experiment 0039: Stage 8 Scrollback and Mouse Copy

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0031](../adr/0031-scrollback-and-mouse-copy.md)

## Scope

Validate bounded client scrollback navigation and focused-Pane mouse selection
through a real interactive compositor PTY.

## Observations

- Ctrl-U moved copy mode into client VT scrollback.
- Select-all over a scrolled viewport captured historical lines and excluded
  newer live lines.
- Mouse press/motion/release in copy mode selected a focused-Pane glyph range
  and wrote it through the explicit clipboard provider.
- Leaving copy mode returned the viewport to live output.
- Snapshot replacement, resize, process exit, and Attach close still cancel
  selection.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-017.

## Limitations

- Scrollback is bounded, in-memory client state and is not persistent history.
- Native terminal selection remains disabled while compositor mouse tracking is
  active.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
