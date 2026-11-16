# ADR 0025: Pane-Scoped Copy and Quick Menu

Status: Accepted
Date: 2026-11-11
Extends: [ADR 0024](0024-pane-composition-controls.md)
Implementation state: Visible-screen copy, explicit clipboard outcomes, and
the client quick menu are implemented.

## Context

After Pane composition exists, terminal-native selection is no longer
correctness-safe: the physical terminal can select text across Pane
boundaries, while the compositor owns the VT grids. Menu keys must not leak to
child programs, and a copy operation must not claim success when no clipboard
provider accepted the text.

## Decision

### 1. Copy mode

`Ctrl-A [` enters copy mode for the focused Pane. Selection coordinates are
relative to that Pane's client-local VT screen and are clamped to it.

- `h/j/k/l` move the copy cursor.
- Space or `v` starts a point selection.
- `V` selects the current visible row.
- `A` selects the full visible screen.
- `y` or Enter copies.
- `q` or Escape cancels.

Only the focused Pane is highlighted and extracted. Native terminal selection
is not used.

The current slice copies visible screen content only, not scrollback. A
terminal snapshot replacement, resize, process exit, or Attach close cancels
an active selection. Worker restart therefore cannot return stale text from a
previous screen generation.

### 2. Clipboard result

The client attempts an explicit provider in this order:

- macOS: `pbcopy`;
- Linux: `wl-copy`, `xclip`, then `xsel`.

The status line reports the provider and byte count on success. Spawn or
nonzero-exit failures are shown as an explicit `copy failed` message; no
silent fallback claims success.

`QWE_CLIPBOARD_COMMAND` is an automation hook that runs through `sh` and
receives the selected text on stdin. It must only be set by a trusted local
environment.

### 3. Quick menu

`Ctrl-A m` opens a client-side menu. The menu offers split, close, left/right
move, copy mode, and menu close.

All menu bytes are consumed by the compositor. Menu actions invoke the same
authorized qwed operations as direct shortcuts. No menu key is forwarded to
PTY input.

## Non-Goals

- Cross-Pane selection or native terminal selection.
- Scrollback copy, persistent selections, or clipboard history.
- Confirming clipboard contents after an asynchronous desktop-manager write.
- Mouse selection or drag behavior.

## Validation

LAYOUT-009 copies visible content from only the focused Pane through a real
clipboard command and observes the success status. LAYOUT-010 verifies menu
keys do not reach the child PTY. Both run in `just stage8-layout`.

## References

- [ADR 0022](0022-session-layout-compositor.md)
- [ADR 0024](0024-pane-composition-controls.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
