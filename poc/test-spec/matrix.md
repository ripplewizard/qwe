# Unified POC Test Matrix

This matrix is the execution checklist for all three implementations. A case is
complete only when its status and evidence are recorded. Manual-only checks
must include reviewer notes; a passing process exit alone is not evidence.

## Scoring Domains

| Domain | Weight | Hard Gate |
|---|---:|---|
| A — Terminal Runtime | 25% | Basic interactive behavior |
| B — Process Runtime | 20% | Reliable process-group control |
| C — Daemon Architecture | 15% | Clean recovery and explicit errors |
| D — Resource Monitoring | 10% | Task-attributable CPU/memory data |
| E — Developer Experience | 10% | Maintainable end-to-end development |
| Cross-platform Capability | 5% | Declared platform gaps |
| Extension Architecture | 5% | Core model remains extensible |
| Dependency and Maintenance | 10% | Acceptable dependency health |

## Test Cases

| ID | Domain | Mode | Evidence |
|---|---|---|---|
| T01 | PTY create/destroy | Auto | FD count, process exit, leak check |
| T02 | Bash/Zsh launch | Auto | Captured output and exit status |
| T03 | Neovim interaction | Manual | Screen recording/notes and terminal size |
| T04 | htop interaction | Manual | Screen recording and update behavior |
| T05 | ANSI/VT coverage | Both | Fixture output plus visual comparison |
| T06 | Unicode and wide text | Both | Exact fixture plus visual comparison |
| T07 | Resize | Both | Four sizes, SIGWINCH and emulator state |
| T08 | Detach/Attach | Manual | PTY, shell, screen, and input state |
| T09 | Session Lease | Auto | Timestamped ownership transitions |
| T10 | Long output | Auto | Sequence integrity and throughput |
| T11 | Job control | Manual | Ctrl+C/Z, `fg`, and `bg` behavior |
| T12 | Terminal daemon restart | Both | Recovery matrix by terminal layer |
| P01 | Single process | Auto | Start, exit code, duration |
| P02 | Multi-level tree | Auto | Process tree and termination |
| P03 | Signals | Auto | SIGINT, SIGTERM, SIGKILL outcomes |
| P04 | Process group | Auto | Group membership and control |
| P05 | Child crash | Auto | Failure detection and task state |
| P06 | Parent exit | Auto | Orphan/child behavior |
| P07 | Timeout | Auto | Deadline and termination evidence |
| P08 | Daemon crash | Auto | Task survival and re-identification |
| P09 | PID reuse | Auto | Stable process identity evidence |
| P10 | Restart policy | Auto | Independent TaskInstance history |
| P11 | cwd and environment | Auto | Captured working directory/env |
| P12 | TaskInstance history | Auto | Query output after restart |
| D01 | CLI/Daemon IPC | Auto | Request ID, response, error model |
| D02 | Concurrent clients | Auto | Load, latency, invariant checks |
| D03 | Forced daemon exit | Auto | Restart and state consistency |
| D04 | SQLite failure | Auto | Corruption, lock, and full-disk cases |
| D05 | Client disconnect | Auto | No leaked or stuck request |
| D06 | Daemon upgrade | Auto | Schema and task compatibility |
| D07 | Idempotency | Auto | Duplicate request/result evidence |
| D08 | Long-running daemon | Auto | Duration, errors, memory trend |
| RM01 | CPU accounting | Auto | Process/group/task values and convention |
| RM02 | Memory accounting | Auto | RSS/PSS/tree methods and limitations |
| RM03 | Sampling overhead | Auto | 100/500/1000/5000 ms samples |
| RM04 | GPU capability | Auto | Supported/Unsupported plus raw metrics |
| RM05 | GPU attribution | Auto | Only evidence-backed task mapping |
| DX01 | CLI and IPC | Auto | Time-to-demo, dependencies, errors |
| DX02 | SQLite and TUI | Both | Persistence check and basic screen |
| DX03 | Build and test workflow | Auto | Build/test time and failure clarity |
| XP01 | Platform matrix | Both | Fedora x86_64 baseline and macOS secondary |
| EX01 | Extension boundaries | Review | Model-impact review against registry |
| MC01 | Dependency health | Review | Licenses, activity, releases, risks |

GPU metrics are capability-gated: unsupported hardware must be reported as
`unsupported`, not scored as failure or silently assigned to a task.

<!-- LATEST_EVIDENCE_START -->

Generated: 2026-11-03T05:29:51Z

| ID | Linux R/G/C | macOS R/G/C | Evidence |
|---|---:|---:|---|
| D01 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D01-b42e2f9-20261103T045651Z-117901.json` |
| D02 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D02-b42e2f9-20261103T045651Z-117901.json` |
| D03 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D03-b42e2f9-20261103T045522Z-112603.json` |
| D04 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D04-b42e2f9-20261103T045651Z-117901.json` |
| D05 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D05-b42e2f9-20261103T045651Z-117901.json` |
| D06 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D06-b42e2f9-20261103T045618Z-117148.json` |
| D07 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D07-b42e2f9-20261103T045522Z-112603.json` |
| D08 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/D08-b42e2f9-20261103T045651Z-117901.json` |
| DX01 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/DX01-b42e2f9-20261103T045807Z-118728.json` |
| DX02 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/DX02-b42e2f9-20261103T045807Z-118728.json` |
| DX03 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/DX03-b42e2f9-20261103T045807Z-118728.json` |
| EX01 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/EX01-4fdc75d-review-123764.json` |
| MC01 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/MC01-4fdc75d-review-123764.json` |
| P01 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P01-b42e2f9-20261103T045619Z-117320.json` |
| P02 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P02-b42e2f9-20261103T045522Z-112603.json` |
| P03 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P03-b42e2f9-20261103T045619Z-117320.json` |
| P04 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P04-b42e2f9-20261103T045619Z-117320.json` |
| P05 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P05-b42e2f9-20261103T045619Z-117320.json` |
| P06 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P06-b42e2f9-20261103T045522Z-112603.json` |
| P07 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P07-b42e2f9-20261103T045612Z-116630.json` |
| P08 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P08-b42e2f9-20261103T045522Z-112603.json` |
| P09 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P09-b42e2f9-20261103T045619Z-117320.json` |
| P10 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P10-b42e2f9-20261103T045616Z-116907.json` |
| P11 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P11-b42e2f9-20261103T045522Z-112603.json` |
| P12 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/P12-b42e2f9-20261103T045619Z-117320.json` |
| RM01 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/RM01-b42e2f9-20261103T045621Z-117618.json` |
| RM02 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/RM02-b42e2f9-20261103T045621Z-117618.json` |
| RM03 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/RM03-b42e2f9-20261103T045621Z-117618.json` |
| RM04 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/RM04-b42e2f9-20261103T045621Z-117618.json` |
| RM05 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/RM05-b42e2f9-20261103T045621Z-117618.json` |
| T01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T01-b42e2f9-20261103T045515Z-111267.json` |
| T02 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T02-b42e2f9-20261103T045515Z-111267.json` |
| T03 | blocked/blocked/blocked | blocked/blocked/blocked | `poc/results/linux-x86_64/rust/T03-b42e2f9-20261103T045515Z-111267.json` |
| T04 | blocked/blocked/blocked | missing/missing/missing | `poc/results/linux-x86_64/rust/T04-b42e2f9-20261103T045604Z-116195.json` |
| T05 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/T05-b42e2f9-20261103T045515Z-111267.json` |
| T06 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/T06-b42e2f9-20261103T045515Z-111267.json` |
| T07 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T07-b42e2f9-20261103T045515Z-111267.json` |
| T08 | blocked/blocked/blocked | missing/missing/missing | `poc/results/linux-x86_64/rust/T08-b42e2f9-20261103T045557Z-116000.json` |
| T09 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/T09-b42e2f9-20261103T045550Z-115748.json` |
| T10 | pass/pass/pass | missing/missing/missing | `poc/results/linux-x86_64/rust/T10-b42e2f9-20261103T045515Z-111267.json` |
| T11 | blocked/blocked/blocked | missing/missing/missing | `poc/results/linux-x86_64/rust/T11-b42e2f9-20261103T045604Z-116195.json` |
| T12 | blocked/blocked/blocked | missing/missing/missing | `poc/results/linux-x86_64/rust/T12-b42e2f9-20261103T045604Z-116195.json` |
| XP01 | fail/fail/fail | fail/fail/fail | `poc/results/linux-x86_64/rust/XP01-b42e2f9-122408.json` |
