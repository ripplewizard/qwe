# Experiment 0030: Stage 8 Layout Foundation

Date: 2026-11-10
Status: Slice 1 pass on Linux x86_64
Related: [ADR 0022](../adr/0022-session-layout-compositor.md),
[Stage 8 Session Layout](../stage8-session-layout.md)

## Scope

Validate Pane-targeted Attach and the first client compositor without adding
durable layout metadata, copy, menus, floating Panes, or a second control
daemon.

## Environment

- Linux x86_64 local host
- Rust workspace in development profile
- Real qwed supervisor, Pane workers, PTYs, SQLite, and Unix sockets

No container result is used as installed-systemd or cross-platform
qualification evidence.

## Observations

`just stage8-layout` created one two-Pane Session and passed:

- LAYOUT-001 deterministic two-Pane tiling;
- LAYOUT-002 Pane-targeted Attach with matching accepted/snapshot identity;
- LAYOUT-003 concurrent exclusive channels to distinct Panes;
- LAYOUT-004 focused raw input delivered only to the selected Pane;
- LAYOUT-005 interactive compose rendering, focus switch, and clean detach.

The existing default-Pane Attach path also passed `just attach-e2e`, and the
Stage 7 multi-Pane inventory/recovery/scope cleanup probe continued to pass.

## Limitations

- Geometry is derived from the current viewport and is not persisted.
- The compositor does not answer terminal-device queries on behalf of child
  programs.
- Copy, quick menu, floating overlays, and shortcut-driven Pane lifecycle
  actions are not implemented.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
