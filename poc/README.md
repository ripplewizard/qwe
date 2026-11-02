# QWE POC Workspace

Status: Stage 1 started. This directory implements the workspace and evidence
rules defined by
[`qwe-technical-poc-specification-v0.1.md`](../docs/qwe-technical-poc-specification-v0.1.md).
Implemented capabilities are PTY creation, Shell launch, initial and dynamic
window sizes, SIGWINCH delivery, and a Neovim rendering preflight. Terminal
Emulator state, Daemon, and recovery behavior are not implemented yet.

## Baseline Decisions

- Official comparison runs use Ubuntu 24.04 LTS, x86_64, at least 4 cores,
  8 GB RAM, and SSD storage.
- macOS ARM64 is a development and preflight environment only. Its results do
  not enter the primary comparison unless the decision report says otherwise.
- Rust, Go, and C++ receive identical test definitions and workloads.
- Each implementation exposes the same POC control contract while retaining
  its native runtime and libraries.
- Raw evidence is retained; scores without linked evidence are invalid.
- POC validation code must exercise real PTYs, processes, IPC, SQLite, and
  recovery paths. Mock backends, fake capability responses, canned success
  output, and sleep-based stand-ins are prohibited.

## Layout

```text
poc/
  README.md                 Preparation state and workflow
  test-spec/                Cases, environment rules, result schema
  test-data/                External workloads used by all implementations
  rust/                     Rust implementation
  go/                       Go implementation
  cpp/                      C++ implementation
  results/                  Evidence split by platform and language
  reports/                  Comparison and final decision
  build/                    Generated binaries; ignored by Git
```

The repository-root [`Justfile`](../Justfile) is the only orchestration entry
point. Shell files under `test-data/scripts/` are real external workloads
consumed by tests, not repository run scripts.

Each language project will gain `terminal/`, `process/`, `daemon/`, and
`resource/` work areas as those stages begin. Do not copy implementation code
between languages; only shared external workloads belong in `test-data/`.

## Preparation Commands

Run from the repository root:

```sh
just doctor                  # Record and inspect toolchain readiness
just build                   # Build all implementations
just build rust              # Build one implementation
just run rust pty-smoke      # Run a prepared implementation
just verify-skeleton         # Verify build and contract parity
just verify-fixtures         # Verify shared workload behavior
just terminal-smoke-macos    # Run terminal tests on macOS
just terminal-smoke-linux    # Run terminal tests on Linux
just test-macos              # Run the macOS validation suite
just test-linux              # Run the Linux validation suite
```

The `pty-smoke` command creates a real operating-system PTY, launches `sh`,
sets an initial 24x80 window, sends a command, and reads the captured output.
It is valid T01/T02 preflight evidence on macOS, but not an official Linux
comparison score.

The `editor-smoke` command runs Neovim in the same PTY, renders a marker sourced
from the child environment, and exits. It is recorded as T03 `blocked` until
manual cursor, scrolling, full-screen, and interaction checks are complete.

The `resize-smoke` command changes the PTY through 120x40, 80x24, 160x50, and
40x15 and verifies both `stty size` and the WINCH trap. macOS requires explicit
SIGWINCH delivery after `TIOCSWINSZ`; Linux uses the native PTY behavior.

Results are written to `results/macos-<arch>/` or `results/linux-<arch>/`.
Runs under the old `results/<language>/` layout predate this split and are
macOS preflight evidence only.

## Execution Order

1. Validate the official Linux environment and freeze toolchain versions.
2. Complete Stage 1: PTY, Shell, Process Group, Signal, IPC, and SQLite.
3. Execute Terminal Runtime tests first; record automated and manual evidence.
4. Run Process and Daemon reliability tests, including crash recovery.
5. Add resource monitoring and developer-experience measurements.
6. Score, review hard gates, and produce the language decision report.

Every run must reference a source commit, environment record, command, result,
logs, and known limitations. Use
[`test-spec/result.schema.json`](test-spec/result.schema.json) for structured
results and [`reports/decision.md`](reports/decision.md) for the final output.

