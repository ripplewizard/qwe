# Linux x86_64 Results

This directory contains Fedora Linux x86_64 runs. Current evidence was produced
on Fedora 35.

Run `just test-linux` on the Fedora host. Final scoring requires a clean source
tree and an environment record confirming Fedora x86_64, at least 4 CPU cores,
8 GB RAM, and the required toolchain versions.

Latest target-platform run: 2026-11-02, source `f44f72e-dirty`.

- T01/T02: Rust, Go, and C++ passed.
- T03: automation passed; status remains `blocked` pending manual interaction.
- T05/T06: all three native terminal parsers passed color, erase, cursor,
  Unicode, emoji, and wide-character state checks.
- T07: Rust, Go, and C++ passed all four dynamic resize and SIGWINCH checks.
- T10: Rust, Go, and C++ preserved all 50,000 output sequence boundaries.
- P02/P03/P08/P09/P12: process tree, process-group stop, daemon crash survival,
  stable identity, and SQLite history passed.
- P01/P05/P11: short-process exit, child-crash detection, cwd/env inheritance,
  and exited-task history passed.
- D01/D03/D07: IPC responses, daemon restart, and idempotent start passed.
- D02/D04/D05: concurrent clients, corrupt SQLite rejection, and client
  disconnect isolation passed. D04 also covers an injected SQLite write error.
- D08: 100 sequential requests over a running-daemon interval passed with RSS
  before/after recorded.

