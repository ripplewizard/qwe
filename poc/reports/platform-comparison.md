# XP01 Platform Capability and Performance Comparison

Status: generated from latest structured result per case and language.

## Metadata

- Generated: 2026-11-03T05:01:57Z
- Source commit: `b42e2f9`
- Primary target: Linux x86_64 (distribution version is metadata).
- Secondary target: macOS ARM64 preflight.
- Selection: latest result by `started_at` for each platform/language/case.

## Coverage Matrix

| Case | Linux Rust | Linux Go | Linux C++ | macOS Rust | macOS Go | macOS C++ |
|---|---:|---:|---:|---:|---:|---:|
| T01 | pass | pass | pass | pass | pass | pass |
| T02 | pass | pass | pass | pass | pass | pass |
| T03 | blocked | blocked | blocked | blocked | blocked | blocked |
| T04 | blocked | blocked | blocked | missing | missing | missing |
| T05 | pass | pass | pass | missing | missing | missing |
| T06 | pass | pass | pass | missing | missing | missing |
| T07 | pass | pass | pass | pass | pass | pass |
| T08 | blocked | blocked | blocked | missing | missing | missing |
| T09 | pass | pass | pass | missing | missing | missing |
| T10 | pass | pass | pass | missing | missing | missing |
| T11 | blocked | blocked | blocked | missing | missing | missing |
| T12 | blocked | blocked | blocked | missing | missing | missing |
| P01 | pass | pass | pass | missing | missing | missing |
| P02 | pass | pass | pass | missing | missing | missing |
| P03 | pass | pass | pass | missing | missing | missing |
| P04 | pass | pass | pass | missing | missing | missing |
| P05 | pass | pass | pass | missing | missing | missing |
| P06 | pass | pass | pass | missing | missing | missing |
| P07 | pass | pass | pass | missing | missing | missing |
| P08 | pass | pass | pass | missing | missing | missing |
| P09 | pass | pass | pass | missing | missing | missing |
| P10 | pass | pass | pass | missing | missing | missing |
| P11 | pass | pass | pass | missing | missing | missing |
| P12 | pass | pass | pass | missing | missing | missing |
| D01 | pass | pass | pass | missing | missing | missing |
| D02 | pass | pass | pass | missing | missing | missing |
| D03 | pass | pass | pass | missing | missing | missing |
| D04 | pass | pass | pass | missing | missing | missing |
| D05 | pass | pass | pass | missing | missing | missing |
| D06 | pass | pass | pass | missing | missing | missing |
| D07 | pass | pass | pass | missing | missing | missing |
| D08 | pass | pass | pass | missing | missing | missing |
| RM01 | pass | pass | pass | missing | missing | missing |
| RM02 | pass | pass | pass | missing | missing | missing |
| RM03 | pass | pass | pass | missing | missing | missing |
| RM04 | pass | pass | pass | missing | missing | missing |
| RM05 | pass | pass | pass | missing | missing | missing |
| DX01 | pass | pass | pass | missing | missing | missing |
| DX02 | pass | pass | pass | missing | missing | missing |
| DX03 | pass | pass | pass | missing | missing | missing |
| EX01 | pass | pass | pass | missing | missing | missing |
| MC01 | pass | pass | pass | missing | missing | missing |

## Performance Snapshot

| Metric | Linux Rust | Linux Go | Linux C++ | macOS Rust | macOS Go | macOS C++ |
|---|---:|---:|---:|---:|---:|---:|
| CLI probe median (ms) | 0.775 | 2.455 | 1.210 | None | None | None |
| IPC status median (ms) | 0.281 | 0.516 | 0.695 | None | None | None |
| Clean build (ms) | 39521.124 | 56564.772 | 4013.412 | None | None | None |
| 50k-line output (ms) | 328 | 331 | 331 | None | None | None |
| Daemon RSS (KiB) | 4464 | 10616 | 5020 | None | None | None |
| 100 ms sample latency (ms) | 112.770 | 112.680 | 113.370 | None | None | None |
| 1000 ms sample latency (ms) | 1012.930 | 1012.380 | 1013.100 | None | None | None |
| 5000 ms sample latency (ms) | 5013.150 | 5016.590 | 5013.680 | None | None | None |

## Capability Differences

- GPU capability flags: [True, True, True, None, None, None].
- Linux exposes task-attributable GPU capability on the current host; macOS reports `unsupported` without a trustworthy process mapping.
- Linux restart identity uses `/proc` process start time; the current macOS token has one-second resolution.
- T12 currently verifies a deliberate boundary: daemon restart does not restore terminal process, PTY, screen, metadata, or input.
- Manual T03/T04/T08/T11/T12 review records are platform evidence, not language-specific runtime scores.

## Missing Evidence

- `macos-arm64/cpp`: T04, T05, T06, T08, T09, T10, T11, T12, P01, P02, P03, P04, P05, P06, P07, P08, P09, P10, P11, P12, D01, D02, D03, D04, D05, D06, D07, D08, RM01, RM02, RM03, RM04, RM05, DX01, DX02, DX03, EX01, MC01
- `macos-arm64/go`: T04, T05, T06, T08, T09, T10, T11, T12, P01, P02, P03, P04, P05, P06, P07, P08, P09, P10, P11, P12, D01, D02, D03, D04, D05, D06, D07, D08, RM01, RM02, RM03, RM04, RM05, DX01, DX02, DX03, EX01, MC01
- `macos-arm64/rust`: T04, T05, T06, T08, T09, T10, T11, T12, P01, P02, P03, P04, P05, P06, P07, P08, P09, P10, P11, P12, D01, D02, D03, D04, D05, D06, D07, D08, RM01, RM02, RM03, RM04, RM05, DX01, DX02, DX03, EX01, MC01

## Interpretation Limits

- Results come from dirty worktrees until the accepted implementation is committed and regenerated.
- Single-host measurements are feasibility data, not production benchmarks.
- `blocked` means manual review or a deliberately unclaimed recovery capability remains; it is not a pass.
- Distribution version is recorded as environment metadata, not a target restriction.
