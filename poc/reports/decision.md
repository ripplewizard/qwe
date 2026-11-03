# Language and Framework Decision

Status: accepted for implementation. Rust is selected for the QWE runtime.
Independent manual sign-off and clean-source evidence regeneration remain POC
record-quality tasks, not implementation blockers.

## Recommendation

Use **Rust** for the QWE core/runtime implementation.

For the first production terminal slice, evaluate **`portable-pty` plus a
maintained VT state engine** (current POC parser: `vt100`; production candidate:
`alacritty_terminal` or equivalent). Use **SQLite** for persistence.

The recommendation is not based on a single benchmark. Rust has the best
combination of:

- equivalent hard-gate results to Go and C++;
- healthy, narrowly scoped PTY/SQLite/VT dependencies;
- consistent typed ownership boundaries for process, daemon, session and
  resource logic;
- no observed runtime penalty that changes the architecture decision.

## Hard Gates

| Gate | Rust | Go | C++ |
|---|---|---|---|
| Basic terminal interaction | Pass* | Pass* | Pass* |
| Reliable process-group control | Pass | Pass | Pass |
| Background/task survival after daemon crash | Pass | Pass | Pass |
| Recoverable TaskInstance identity | Pass* | Pass* | Pass* |
| Explicit IPC/persistence errors | Pass | Pass | Pass |
| Acceptable dependency health | Pass | Pass with risk | Pass |

`Pass*` is waiting for independent human terminal review sign-off.

## Why Rust

- Weighted provisional score: **3.95**, versus Go **3.40** and C++ **3.10**.
- Rust won no hard-gate count by accident: all three implementations passed
  process, daemon, SQLite, resource and developer-experience checks.
- Rust's advantage is engineering economy and dependency quality. Its four
  direct dependencies have clear licenses and current maintenance signals.
- The POC did not expose a Rust-specific terminal failure. The remaining T12
  terminal-loss behavior is shared by all three implementations.

## Why Not Go as the Primary Direction

Go is a credible fallback and passed every hard gate. Its remaining weakness is
the VT state dependency: `github.com/hinshun/vt10x` is pinned to a 2022
pseudo-version, has low recent activity, no release tags, and only one open
issue. A Go production direction would require replacing or owning that parser.
The cgo SQLite dependency also adds a native toolchain requirement.

## Why Not C++ as the Primary Direction

C++ is technically capable and has the fastest clean build in this POC. It did
not provide a runtime advantage large enough to offset:

- manual PTY/socket/thread/process ownership;
- direct libvterm screen callback integration;
- more low-level error and lifetime handling;
- higher cross-platform build/link adaptation cost.

C++ remains appropriate only if an external requirement mandates it.

## Architecture Decision Required Before Production

The POC does not restore a terminal after daemon restart. Do not reinterpret
P08 task survival as terminal recovery. Before implementing the production
terminal service, choose one:

1. **Durable terminal host:** separate PTY/session owner that survives daemon
   restart and exposes versioned attach/input/state operations.
2. **Explicit terminal-loss semantics:** task processes survive, but terminal
   sessions are documented as restart-scoped.

For the durable option, persist compact VT state (screen, cursor, modes,
bounded scrollback) and only a bounded recent byte journal rather than replaying
the entire raw stream.

## Required Follow-up

1. Obtain independent human sign-off for T03, T04, T08, T11, and the T12
   boundary.
2. Commit the accepted code, regenerate both platform evidence, and confirm no
   source file is dirty.
3. Prototype the durable terminal host and compact VT-state attach.
4. Version the IPC contract before Mesh or extension integration.
5. Repeat performance measurements with multiple clean runs and report median,
   P95, maximum and standard deviation.

## Rejection Summary

| Candidate | Decision | Reason |
|---|---|---|
| Rust | Recommended provisionally | Best dependency and integration balance with all hard gates passing |
| Go | Viable fallback | Hard gates pass; stale VT parser and cgo are material adoption costs |
| C++ | Not preferred | Hard gates pass; lowest developer-experience score and highest manual integration surface |
