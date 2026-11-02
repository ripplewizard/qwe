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
| XP01 | Platform matrix | Both | Linux x86_64 baseline and macOS secondary |
| EX01 | Extension boundaries | Review | Model-impact review against registry |
| MC01 | Dependency health | Review | Licenses, activity, releases, risks |

GPU metrics are capability-gated: unsupported hardware must be reported as
`unsupported`, not scored as failure or silently assigned to a task.

<!-- LATEST_EVIDENCE_START -->
## Latest Evidence Snapshot

Generated from the newest structured result per platform, language, and
case. Manual cells remain `blocked` until independent human sign-off.

- Generated: 2026-11-02T17:50:49Z

| ID | Linux R/G/C | macOS R/G/C | Evidence |
|---|---:|---:|---|
| T01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T01-9abaff7-dirty-20261102T174601Z-94989.json` |
| T02 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T02-9abaff7-dirty-20261102T174601Z-94989.json` |
| T03 | blocked/blocked/blocked | blocked/blocked/blocked | `poc/results/linux-x86_64/manual/T03.md`, `poc/results/macos-arm64/manual/T03.md` |
| T04 | blocked/blocked/blocked | blocked/blocked/blocked | `poc/results/linux-x86_64/manual/T04.md`, `poc/results/macos-arm64/manual/T04.md` |
| T05 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T05-9abaff7-dirty-20261102T174601Z-94989.json` |
| T06 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T06-9abaff7-dirty-20261102T174601Z-94989.json` |
| T07 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T07-9abaff7-dirty-20261102T174601Z-94989.json` |
| T08 | blocked/blocked/blocked | blocked/blocked/blocked | `poc/results/linux-x86_64/manual/T08.md`, `poc/results/macos-arm64/manual/T08.md` |
| T09 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T09-9abaff7-dirty-20261102T174622Z-99337.json` |
| T10 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/T10-9abaff7-dirty-20261102T174601Z-94989.json` |
| T11 | blocked/blocked/blocked | blocked/blocked/blocked | `poc/results/linux-x86_64/manual/T11.md`, `poc/results/macos-arm64/manual/T11.md` |
| T12 | blocked/blocked/blocked | blocked/blocked/blocked | `poc/results/linux-x86_64/manual/T12.md`, `poc/results/macos-arm64/manual/T12.md` |
| P01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P01-9abaff7-dirty-20261102T174650Z-100946.json` |
| P02 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P02-9abaff7-dirty-20261102T174606Z-96220.json` |
| P03 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P03-9abaff7-dirty-20261102T174650Z-100946.json` |
| P04 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P04-9abaff7-dirty-20261102T174650Z-100946.json` |
| P05 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P05-9abaff7-dirty-20261102T174650Z-100946.json` |
| P06 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P06-9abaff7-dirty-20261102T174606Z-96220.json` |
| P07 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P07-9abaff7-dirty-20261102T174643Z-100250.json` |
| P08 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P08-9abaff7-dirty-20261102T174606Z-96220.json` |
| P09 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P09-9abaff7-dirty-20261102T174650Z-100946.json` |
| P10 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P10-9abaff7-dirty-20261102T174647Z-100530.json` |
| P11 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P11-9abaff7-dirty-20261102T174606Z-96220.json` |
| P12 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/P12-9abaff7-dirty-20261102T174650Z-100946.json` |
| D01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D01-9abaff7-dirty-20261102T174714Z-101502.json` |
| D02 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D02-9abaff7-dirty-20261102T174714Z-101502.json` |
| D03 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D03-9abaff7-dirty-20261102T174606Z-96220.json` |
| D04 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D04-9abaff7-dirty-20261102T174714Z-101502.json` |
| D05 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D05-9abaff7-dirty-20261102T174714Z-101502.json` |
| D06 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D06-9abaff7-dirty-20261102T174649Z-100771.json` |
| D07 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D07-9abaff7-dirty-20261102T174606Z-96220.json` |
| D08 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/D08-9abaff7-dirty-20261102T174714Z-101502.json` |
| RM01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/RM01-9abaff7-dirty-20261102T174652Z-101249.json` |
| RM02 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/RM02-9abaff7-dirty-20261102T174652Z-101249.json` |
| RM03 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/RM03-9abaff7-dirty-20261102T174652Z-101249.json` |
| RM04 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/RM04-9abaff7-dirty-20261102T174652Z-101249.json` |
| RM05 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/RM05-9abaff7-dirty-20261102T174652Z-101249.json` |
| DX01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/DX01-9abaff7-dirty-20261102T174825Z-102321.json` |
| DX02 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/DX02-9abaff7-dirty-20261102T174825Z-102321.json` |
| DX03 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/DX03-9abaff7-dirty-20261102T174825Z-102321.json` |
| XP01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/XP01-9abaff7-dirty-73516.json` |
| EX01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/EX01-9abaff7-dirty-review-73478.json` |
| MC01 | pass/pass/pass | pass/pass/pass | `poc/results/linux-x86_64/rust/MC01-9abaff7-dirty-review-73478.json` |
<!-- LATEST_EVIDENCE_END -->

