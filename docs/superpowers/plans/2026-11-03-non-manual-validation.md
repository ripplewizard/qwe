# Complete Non-Manual POC Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans
> to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for
> tracking. Manual verification remains explicitly blocked.

**Goal:** Complete every automated or machine-checkable POC matrix item for
Rust, Go, and C++ on Linux x86_64 and macOS ARM64.

**Architecture:** Extend the existing comparable POC binaries and shared Python
test runners. Each test exercises real PTYs, processes, Unix sockets, SQLite,
and operating-system metrics. Results must validate against
`poc/test-spec/result.schema.json`.

**Tech Stack:** Rust, Go, C++20, POSIX process/PTY APIs, SQLite, Python 3,
Just, macOS `ps`/`proc` facilities, Linux `/proc`.

**Spec:** `docs/qwe-technical-poc-specification-v0.1.md`,
`poc/test-spec/matrix.md`, and the per-domain contracts under
`poc/test-spec/`.

## Global Constraints

- Target execution platform is Linux x86_64; distribution version is metadata.
- macOS ARM64 is secondary preflight evidence.
- No mocked PTY, process, IPC, SQLite, or resource values may claim acceptance.
- Every automated case records source commit, environment, command, duration,
  metrics, boolean checks, evidence, and limitations.
- Manual T03/T04/T08/T11/T12 observations remain blocked until reviewed by a
  human; only their automated preflights are added here.
- Preserve P07's exact deadline/termination semantics and P10's independent
  TaskInstance history.

## Review Focus

- Exit-code and signal races can report the wrong final task state.
- PID reuse detection must not rely on PID alone.
- Resource sums can double-count shared memory and must state their convention.
- GPU metrics must be `unsupported` when no trustworthy capability or process
  mapping exists.
- Long-running tests must distinguish sustained stability from short smoke runs.

---

### Task 1: Complete Process and Task Acceptance

- [x] Add persisted exit code, start/end duration, and failed state to normal
  task execution.
- [x] Add SIGINT, SIGTERM, and SIGKILL task operations with exact exit semantics.
- [x] Add dedicated P04 process-group membership/control verification.
- [x] Simulate P09 PID reuse by placing an unrelated process/start-token pair in
  the database and verifying recovery rejects it.
- [x] Expose P12 TaskInstance history and verify it after daemon restart.
- [x] Keep schema v2 compatible; normal task history uses the existing
  `task_instances` table without a destructive migration.
- [x] Run P01/P03/P04/P05/P09/P12/D06 on Rust, Go, C++, Linux, and macOS.

### Task 2: Complete Terminal Automated Coverage

- [x] T01: measure FD count before/after and verify child reaping.
- [x] T02: launch and verify both Bash and Zsh.
- [x] T05: add alternate screen, scroll region, cursor visibility, erase-display,
  and title assertions.
- [x] T06: expand CJK, emoji, combining marks, and wide-character assertions.
- [x] T07: verify parser/emulator dimensions after each resize.
- [ ] Add automated T03/T04/T11/T12 preflights, leaving their manual review
  status blocked.

### Task 3: Resource Monitoring

- [x] RM01: implement task/process-group CPU time and normalized CPU usage.
- [x] RM02: implement process-tree RSS and document shared-memory limitations.
- [x] RM03: measure 100/500/1000/5000 ms sampling overhead.
- [x] RM04: report backend-supported raw GPU metrics or `unsupported`.
- [x] RM05: attribute GPU usage to a TaskInstance only with reliable PID mapping.

### Task 4: Daemon Failure Depth and Long Running

- [ ] D01: cover invalid requests, not-found, request-ID, and stable error codes.
- [ ] D02: record concurrency latency and invariant checks.
- [ ] D04: cover lock contention, injected disk-full behavior, and corruption.
- [ ] D05: verify no stuck request or leaked client after disconnect.
- [ ] D08: run a sustained interval and record request errors plus RSS trend.

### Task 5: Developer Experience and Platform Metrics

- [ ] DX01: record CLI time-to-demo, dependency count, and error clarity.
- [ ] DX02: add a minimal CLI/TUI screen and persistence check.
- [ ] DX03: measure clean build/test time and capture failure output.
- [ ] XP01: generate comparative Linux/macOS capability and performance data.

### Task 6: Extension and Dependency Reviews

- [ ] EX01: review extension registry against implemented object boundaries.
- [ ] MC01: record licenses, release activity, maintenance risk, and platform
  availability for every selected dependency.
- [ ] Regenerate the automated summary and update every matrix row with evidence.

## Verification

- [ ] `just fmt`
- [ ] `just lint`
- [ ] `just verify-skeleton`
- [ ] `just verify-fixtures`
- [ ] All automated smoke recipes on Linux and macOS
- [ ] Full result-schema and evidence-path validation
