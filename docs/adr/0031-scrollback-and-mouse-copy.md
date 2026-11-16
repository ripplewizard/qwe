# ADR 0031: Scrollback and Mouse Copy

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0025](0025-copy-and-quick-menu.md)
Implementation state: Client scrollback copy and focused-Pane mouse selection
are implemented.

## Context

ADR 0025 limited copy mode to the visible screen. The client VT parser already
retains bounded scrollback, so copy mode can expose historical rows without
persisting terminal captures or creating a replay protocol.

## Decision

### 1. Scrollback navigation

Copy mode tracks a scrollback offset in `CopyState`.

- Ctrl-U scrolls one row toward older history.
- Ctrl-D scrolls one row toward the live viewport.
- Offset is clamped to the focused Pane's client VT scrollback.
- Active selection is cleared when the scrollback offset changes.
- Rendering temporarily applies the offset, then returns all non-copy views to
  the live viewport.

Scrollback remains in-memory, client-local, and bounded. It is not the worker
journal and does not survive Attach replacement or a compositor restart.

### 2. Mouse selection

While in copy mode, left mouse press in the focused Pane content starts a
selection; motion updates the endpoint; release copies the selected text and
exits copy mode. Title rows and cells outside the focused Pane are ignored.

Mouse selection is clamped to the focused Pane and uses the same clipboard
provider and explicit result reporting as keyboard copy.

### 3. State replacement

Snapshot replacement, resize, process exit, Attach close, or compositor restart
cancels active selection. Selection never outlives the VT screen generation it
was created against.

## Validation

LAYOUT-016 scrolls into history, selects all visible historical text, and
verifies newer live lines are excluded. LAYOUT-017 uses SGR mouse
press/motion/release to select text in copy mode and verifies the clipboard.

## References

- [ADR 0025](0025-copy-and-quick-menu.md)
- [ADR 0028](0028-mouse-drag-and-hit-testing.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
