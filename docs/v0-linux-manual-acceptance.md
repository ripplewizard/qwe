# V0 Linux Manual Acceptance

Status: Pending independent reviewer
Date: 2026-11-06

This is the final human gate for the Linux-first V0 release. Automated release
and security gates must pass before this review starts.

## Candidate

- Branch: `codex/stage5-linux-release`
- Candidate commit: `5e3197a`
- Platform: Fedora Linux x86_64
- Release archive: `dist/qwe-0.1.0-linux-x86_64.tar.gz`
- Archive SHA-256: `89b53822144ea26d35a6d3b7001073d3ad520cfb6cc7386df2f70f54d47c1514`
- Reviewer:
- Reviewer independent from implementation: yes/no
- Review date:
- Terminal emulator:

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
| T03 Neovim rendering and interaction | Pending | | |
| T04 htop render, refresh, resize, exit | Pending | | |
| T08 Neovim detach, delayed output, reattach | Pending | | |
| T11 Ctrl-C/Z, `jobs`, `bg`, `fg`, reattach | Pending | | |
| Package install and launchd/systemd startup | Pending | | |
| Detach and reattach without gaps/duplicates | Pending | | |
| Terminal modes restored after detach | Pending | | |

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
| Archive checksum verified | Pending | |
| Installation succeeds without checkout path | Pending | |
| Installed `qwe`/`qwed` versions match manifest | Pending | |
| systemd starts installed qwed | Pending | |
| Package reinstall preserves live Session | Pending | |
| qwed crash restart adopts same worker/task | Pending | |
| Stop leaves durable `closed`/`stopped` state | Pending | |
| Documentation is sufficient for a new user | Pending | |

## Sign-off

```text
Reviewer:
Independent: yes/no
Result: PASS/FAIL
Reviewed commit:
Archive SHA-256:
Date:
```

Stage 5 cannot be closed until this section records an independent PASS.
