# Experiment 0011: Terminal Keyboard Protocol Reset on Detach

Status: Passed on macOS ARM64 and Fedora Linux x86_64
Date: 2026-11-05

## Problem

Neovim running through Attach can enable the Kitty keyboard protocol in the
local terminal emulator. Detaching in iTerm2 left that protocol active, so
iTerm2 continued emitting key-release events such as `CSI 93;5;3u` into the
local shell.

The previous CLI reset colors, cursor, bracketed paste, alternate screen, and
mouse modes, but did not reset:

- Kitty keyboard protocol flags.
- Xterm modified-key reporting.
- Synchronized-output mode.

## Implementation

`qwe` now emits a comprehensive reset sequence on every Attach exit:

- Default colors and cursor style.
- Cursor visibility, application cursor/keypad, and autowrap modes.
- Bracketed paste, synchronized output, and alternate screen.
- Mouse tracking and SGR mouse encoding.
- Kitty keyboard stack pop and explicit zero flags.
- Xterm `modifyOtherKeys` reset.

The reset remains best-effort terminal output; unsupported private sequences
are ignored by terminals that do not implement them.

## Regression Gate

`just terminal-reset-smoke` runs the real CLI under a controlled PTY:

1. Enables Kitty keyboard reporting from the remote shell.
2. Detaches with `Ctrl-]`.
3. Verifies the CLI emits the Kitty and modified-key reset sequences.

The probe has stable case ID `ATTACH-015` and is part of `just stage2-check`.

## Results

- `cargo test -p qwe`: pass.
- `just terminal-reset-smoke` on macOS ARM64: pass.
- `just terminal-reset-smoke` on Fedora x86_64: pass.
- `just stage2-check` on Fedora x86_64: pass.

## Remaining

- Recheck the manual iTerm2 T08 scenario on the next interactive review.
