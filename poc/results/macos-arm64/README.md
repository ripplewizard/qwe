# macOS ARM64 Results

These are macOS preflight results. They verify behavior and expose platform
differences but do not provide official Fedora x86_64 comparison scores.

The terminal and runtime recovery slices pass for Rust, Go, and C++. T03 remains
blocked pending manual interaction.

T05/T06 pass through Rust `vt100`, Go `vt10x`, and C++ libvterm.

Runtime coverage includes concurrent IPC, partial-client disconnect isolation,
rejection of a corrupt SQLite database, and injected SQLite write errors.
T10 preserves all 50,000 long-output sequence boundaries.

P01/P05/P11 cover short process history, child crash detection, and cwd/env
inheritance. D08 records 100 requests and daemon RSS before/after.

The PTY resize implementation sends an explicit SIGWINCH to the foreground
process group after `TIOCSWINSZ`. This avoids driver-specific delivery
differences between macOS and Linux.

