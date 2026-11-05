# Stage 2 Manual TUI Review

These scripts prepare a real qwed/worker/PTY environment and launch one manual
case at a time. They do not replace independent review and do not grade the
result automatically.

## Fedora Setup

On `istoria`:

```sh
cd ~/project/uranus/bianca
scripts/manual-stage2-tui/start-review.sh
```

Run the review over an interactive terminal. Record the platform, terminal
emulator, source commit, reviewer name, and local recording/screenshot paths.

## Cases

Run each attached command from the same repository checkout:

```sh
scripts/manual-stage2-tui/t03-nvim.sh
scripts/manual-stage2-tui/t04-htop.sh
scripts/manual-stage2-tui/t08-detach-reattach.sh
scripts/manual-stage2-tui/t11-job-control.sh
```

Each script prints its checklist before replacing itself with the Attach CLI.
It also prints and persists the actual SessionId under the state directory, so
detach/reattach uses the same running Session. If an older terminal Session
with the base name exists, the script creates a timestamped Session instead of
failing with `already_exists`.

For `t08-detach-reattach.sh`, follow the on-screen delayed-output command, press
`Ctrl-]`, wait at least six seconds, then run the same script again to reattach.

## Status and Cleanup

```sh
scripts/manual-stage2-tui/status.sh
scripts/manual-stage2-tui/stop-review.sh
```

The state directory defaults to:

```text
/tmp/qwe-stage2-manual-$(id -u)
```

Override it with `QWE_MANUAL_STATE_ROOT` before starting if required.

## Terminal Requirements

`Ctrl-]` must arrive as ASCII `0x1d`. Verify with:

```sh
old="$(stty -g)"
stty -icanon -echo min 1 time 0
od -An -tx1 -N1
stty "$old"
```

If the result is not `1d`, fix the terminal key mapping before reviewing T08.
On macOS, `Terminal.app` has passed the current review path; a known iTerm2
Kitty-keyboard-protocol restoration defect is still tracked separately.

## Evidence

Keep raw recordings, terminal captures, SQLite files, and logs on the review
host. Commit only a concise signed report with the reviewer identity, source
commit, platform, observations, and local evidence paths.
