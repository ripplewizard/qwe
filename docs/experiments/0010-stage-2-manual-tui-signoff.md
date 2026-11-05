# Experiment 0010: Stage 2 Manual TUI Sign-off

Status: Passed
Date: 2026-11-05
Reviewer: hjian
Independence: Reviewer confirmed independence from the runtime implementation.
Reviewed revision: `c16a557` (`dev`)

## Scope

The reviewer manually exercised the production Rust TUI path on macOS ARM64
and Fedora Linux x86_64:

| Case | Scenario |
|---|---|
| T03 | Neovim visual rendering and interaction |
| T04 | htop rendering, refresh, interaction, and resize |
| T08 | Neovim detach, background continuation, and reattach |
| T11 | Bash Ctrl-C/Z, `jobs`, `bg`, `fg`, and background state across Attach |

Automated preflight passed before manual review:

```sh
just doctor
just stage2-check
just tui-attach-smoke
```

## Results

| Platform | Terminal | T03 | T04 | T08 | T11 |
|---|---|---|---|---|---|
| macOS 26.5.1 ARM64 | Terminal.app | PASS | PASS | PASS | PASS |
| Fedora 35 x86_64 | interactive Bash terminal | PASS | PASS | PASS | PASS |

Observed behavior included:

- Neovim full-screen layout, line display, cursor, status line, scrolling,
  split windows, Unicode text, and resize redraw.
- htop CPU/memory/Tasks rendering, dynamic refresh, interaction, resize,
  detach/reattach, and clean exit.
- Detached Neovim output was retained and visible after reattach; new input was
  accepted without duplicated or missing output.
- Bash Ctrl-Z, `jobs`, `bg`, `fg`, Ctrl-C, multiple background jobs, and
  background state across detach/reattach.

Evidence was retained locally by the reviewer. Raw terminal recordings and
screenshots are intentionally not committed.

## Known Terminal Defect

iTerm2 exposed a separate compatibility defect: Neovim can enable the Kitty
keyboard protocol, but the CLI does not yet explicitly reset that protocol on
detach. iTerm2 then emits key-release sequences such as `CSI 93;5;3u` into the
local shell.

The same T08 flow passes in Terminal.app, and the required Fedora/macOS manual
matrix therefore passes. The iTerm2 terminal-state restoration issue remains
an explicit follow-up and must not be reported as fixed.

## Sign-off

```text
Reviewer: hjian
Independent: yes
Result: Stage 2 manual TUI acceptance PASS
Reviewed commit: c16a557
Date: 2026-11-05
```
