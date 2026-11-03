# Remaining Validation Before Final Selection

Status: implementation approved; POC evidence sign-off remains pending.

## Completed in This Pass

- T03 automated Neovim preflight and full-screen interaction record.
- T04 htop launch, dynamic refresh, resize, and clean-exit preflight.
- T11 Ctrl+C, Ctrl+Z, `jobs`, `bg`, and `fg` preflight.
- T12 daemon-restart layer-by-layer boundary record.
- D04 corruption, lock contention, and real isolated-filesystem ENOSPC.
- DX01 CLI/IPC timing, dependency count, and error clarity.
- DX02 minimal alternate-screen TUI backed by persisted SQLite.
- DX03 clean-build timing and compiler-failure clarity.
- XP01 Linux/macOS capability and performance comparison.
- EX01 extension-registry/object-boundary review.
- MC01 dependency license, activity, release, platform, and risk review.

Every matrix row now has a generated latest-evidence entry. The latest 258
structured results (43 cases, 3 languages, 2 platforms) pass the result schema
and referenced evidence-path checks.

## Manual Records

T03, T04, T08, and T11 have platform-level observation notes for both Linux and
macOS, covering all three implementations where applicable. T12 has a complete
negative recovery matrix.

Independent human sign-off is still required before changing those cases from
`blocked` to `pass`. This is a process requirement, not missing automation.

## T12 Finding

Killing the daemon terminates the current terminal process and loses session
metadata, PTY ownership, screen state, and input. Restarting the daemon against
the same socket/database succeeds, but the old session is `not_found`.

This does not block the current language recommendation, because all three
implementations share the gap, but it blocks a production claim of terminal
recovery. Before production, select a durable terminal host or explicitly
adopt terminal-loss semantics.

## Clean Evidence Prerequisite

Current evidence is source revision `9abaff7-dirty`. After the accepted code and
reports are committed:

1. Rerun Linux and macOS suites from the committed revision.
2. Regenerate XP01, EX/MC structured review results, and matrix evidence rows.
3. Confirm the source worktree has no tracked modifications outside generated
   evidence.
4. Record independent manual sign-off.

These steps close the POC audit trail. They no longer block Rust implementation,
which proceeds under Baseline v0.3 and ADR 0001-0005.
