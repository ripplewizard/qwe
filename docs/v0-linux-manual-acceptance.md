# V0 Linux Manual Acceptance

Status: Passed
Date: 2026-11-06

This is the final human gate for the Linux-first V0 release. Automated release
and security gates must pass before this review starts.

## Candidate

- Branch: `codex/stage5-linux-release`
- Candidate commit: `8d294df`
- Platform: Fedora Linux x86_64
- Release archive: `dist/qwe-0.1.0-linux-x86_64.tar.gz`
- Archive SHA-256: `7835ddc5a23e8c85be88b01051b6415493d34b6b09b5231084ea3e704cc681bf`
- Operator: Codex agent, under the user's explicit instruction
- Operator independent from implementation: no
- Reviewer and submitter: hjian
- Reviewer independent from implementation: yes
- Review date: 2026-11-06
- Terminal environment: SSH + tmux, 100x30, resized to 120x35

## Automated Preflight

Run from `/home/project/uranus/bianca`:

```sh
just release-gate
just stage4-check
```

Both commands must pass before continuing.

## Manual Cases

Start the review environment:

```sh
scripts/manual-stage2-tui/start-review.sh
```

| Case | Result | Evidence location | Notes |
|---|---|---|---|
| T03 Neovim rendering and interaction | PASS | `artifacts/stage5-linux-manual-20261106/t03-*.txt` | Unicode text, cursor, scrollback, split, and resize rendered correctly |
| T04 htop render, refresh, resize, exit | PASS | `artifacts/stage5-linux-manual-20261106/t04-*.txt` | CPU, memory, tasks, process list, dynamic refresh, resize, and exit worked |
| T08 Neovim detach, delayed output, reattach | PASS | `artifacts/stage5-linux-manual-20261106/t08-delayed-output.txt` | Delayed output remained visible after reattach |
| T11 Ctrl-C/Z, `jobs`, `bg`, `fg`, reattach | PASS | `artifacts/stage5-linux-manual-20261106/t11-*.txt` | Stopped/running/foreground and background-job reattach states matched |
| Package install and systemd startup | PASS | [Experiment 0022](experiments/0022-stage5-linux-release.md) | PACK-003/PACK-005 passed |
| Detach and reattach without gaps/duplicates | PASS | T03/T08 captures | No duplicate or missing visible content |
| Terminal modes restored after detach | PASS | T03/T11 captures | Local shell and job control remained usable after detach |

Use:

```sh
scripts/manual-stage2-tui/t03-nvim.sh
scripts/manual-stage2-tui/t04-htop.sh
scripts/manual-stage2-tui/t08-detach-reattach.sh
scripts/manual-stage2-tui/t11-job-control.sh
scripts/manual-stage2-tui/status.sh
scripts/manual-stage2-tui/stop-review.sh
```

Raw terminal captures, recordings, and SQLite files remain on the review host.
Record only their local paths here.

## Release Acceptance

| Check | Result | Notes |
|---|---|---|
| Archive checksum verified | PASS | `7835ddc5...cc681bf` |
| Installation succeeds without checkout path | PASS | PACK-003 |
| Installed `qwe`/`qwed` versions match manifest | PASS | PACK-003 |
| systemd starts installed qwed | PASS | PACK-005 |
| Package reinstall preserves live Session | PASS | PACK-005 |
| qwed crash restart adopts same worker/task | PASS | PACK-005 |
| Stop leaves durable `closed`/`stopped` state | PASS | PACK-005 |
| Documentation commands and paths resolve | PASS | New-user comprehension remains part of independent review |

## Sign-off

```text
Operator: Codex agent
Reviewer and submitter: hjian
Independent: yes
Result: PASS
Reviewed commit: 8d294df
Archive SHA-256: 7835ddc5a23e8c85be88b01051b6415493d34b6b09b5231084ea3e704cc681bf
Date: 2026-11-06
```

The functional manual confirmation and independent sign-off are complete.
