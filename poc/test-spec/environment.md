# Test Environment Contract

## Official Baseline

The primary comparison environment is Linux x86_64:

- Any maintained Linux distribution is acceptable; record the exact
  distribution, release, kernel, and toolchain versions
- At least 4 CPU cores, 8 GB RAM, and SSD storage
- Bash, Zsh, Neovim, htop, SQLite, jq, Python 3, Ninja, and Git
- Recorded Rust, Go, C++ compiler, CMake/Meson, kernel, CPU, and memory versions

The current macOS ARM64 host is suitable for scaffold and preflight checks. It
is a secondary environment and must not supply primary comparison scores based
on the Linux target.

## Run Record

Every test record includes:

```text
Test ID, source commit, language/compiler version, dependency lock/build mode,
command, runtime environment, result, duration, logs, screenshots if relevant,
and known limitations.
```

Run `just doctor` before testing. Save its output beside the raw evidence. A
dirty worktree must be labelled as such; results from uncommitted code cannot
be final scoring evidence.

Use `just test-macos` for macOS preflight and `just test-linux` on the Linux
comparison host. Results are written to platform-specific directories and must
not be combined.

## Reset Rules

- Start every performance run from a clean daemon state.
- Remove temporary SQLite databases, sockets, PTYs, and child processes.
- Use the same CPU, memory, and GPU workload scripts for all languages.
- Repeat timing measurements at least five times and report median, P95,
  maximum, and standard deviation where useful.
- Separate automated pass/fail checks from manual visual observations.

## Evidence Integrity

The POC executable and validation path must not fake behavior. Any apparent
PTY, process, IPC, persistence, recovery, or monitoring result must come from
the implementation being evaluated or the actual operating system.

Prohibited evidence includes hardcoded success responses, mocked system calls,
fake Daemon state, canned metrics, and sleeping for a fixed duration in place of
an implemented operation. If a platform lacks a capability, record
`unsupported`; if implementation is incomplete, record `blocked` or `not-run`.
Unit-test doubles may not be used to claim POC acceptance.

