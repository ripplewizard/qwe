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
| XP01 | Platform matrix | Both | Linux baseline and macOS secondary |
| EX01 | Extension boundaries | Review | Model-impact review against registry |
| MC01 | Dependency health | Review | Licenses, activity, releases, risks |

GPU metrics are capability-gated: unsupported hardware must be reported as
`unsupported`, not scored as failure or silently assigned to a task.

