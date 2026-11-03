# XP01 Platform Capability and Performance Comparison

Status: generated from latest structured result per case and language.

## Metadata

- Generated: 2026-11-03T09:10:11Z
- Source commit: `46b5092`
- Primary target: Linux x86_64 (distribution version is metadata).
- Secondary target: macOS ARM64 preflight.
- Selection: latest result by `started_at` for each platform/language/case.

## Coverage Matrix

| Case | Linux Rust | Linux Go | Linux C++ | macOS Rust | macOS Go | macOS C++ |
|---|---:|---:|---:|---:|---:|---:|
| T01 | pass | pass | pass | pass | pass | pass |
| T02 | pass | pass | pass | pass | pass | pass |
| T03 | blocked | blocked | blocked | blocked | blocked | blocked |
| T04 | blocked | blocked | blocked | blocked | blocked | blocked |
| T05 | pass | pass | pass | pass | pass | pass |
| T06 | pass | pass | pass | pass | pass | pass |
| T07 | pass | pass | pass | pass | pass | pass |
| T08 | blocked | blocked | blocked | blocked | blocked | blocked |
| T09 | pass | pass | pass | pass | pass | pass |
| T10 | pass | pass | pass | pass | pass | pass |
| T11 | blocked | blocked | blocked | blocked | blocked | blocked |
| T12 | blocked | blocked | blocked | blocked | blocked | blocked |
| P01 | pass | pass | pass | pass | pass | pass |
| P02 | pass | pass | pass | pass | pass | pass |
| P03 | pass | pass | pass | pass | pass | pass |
| P04 | pass | pass | pass | pass | pass | pass |
| P05 | pass | pass | pass | pass | pass | pass |
| P06 | pass | pass | pass | pass | pass | pass |
| P07 | pass | pass | pass | pass | pass | pass |
| P08 | pass | pass | pass | pass | pass | pass |
| P09 | pass | pass | pass | pass | pass | pass |
| P10 | pass | pass | pass | pass | pass | pass |
| P11 | pass | pass | pass | pass | pass | pass |
| P12 | pass | pass | pass | pass | pass | pass |
| D01 | pass | pass | pass | pass | pass | pass |
| D02 | pass | pass | pass | pass | pass | pass |
| D03 | pass | pass | pass | pass | pass | pass |
| D04 | pass | pass | pass | pass | pass | pass |
| D05 | pass | pass | pass | pass | pass | pass |
| D06 | pass | pass | pass | pass | pass | pass |
| D07 | pass | pass | pass | pass | pass | pass |
| D08 | pass | pass | pass | pass | pass | pass |
| RM01 | pass | pass | pass | pass | pass | pass |
| RM02 | pass | pass | pass | pass | pass | pass |
| RM03 | pass | pass | pass | pass | pass | pass |
| RM04 | pass | pass | pass | pass | pass | pass |
| RM05 | pass | pass | pass | pass | pass | pass |
| DX01 | pass | pass | pass | pass | pass | pass |
| DX02 | pass | pass | pass | pass | pass | pass |
| DX03 | pass | pass | pass | pass | pass | pass |
| EX01 | pass | pass | pass | pass | pass | pass |
| MC01 | pass | pass | pass | pass | pass | pass |

## Performance Snapshot

| Metric | Linux Rust | Linux Go | Linux C++ | macOS Rust | macOS Go | macOS C++ |
|---|---:|---:|---:|---:|---:|---:|
| CLI probe median (ms) | 0.775 | 2.455 | 1.210 | 2.392 | 4.571 | 2.532 |
| IPC status median (ms) | 0.281 | 0.516 | 0.695 | 0.045 | 0.050 | 0.069 |
| Clean build (ms) | 39521.124 | 56564.772 | 4013.412 | 14486.140 | 15147.347 | 1811.362 |
| 50k-line output (ms) | 328 | 331 | 331 | 357 | 339 | 333 |
| Daemon RSS (KiB) | 4464 | 10616 | 5020 | 4304 | 15856 | 2832 |
| 100 ms sample latency (ms) | 112.770 | 112.680 | 113.370 | 131.190 | 124.850 | 137.150 |
| 1000 ms sample latency (ms) | 1012.930 | 1012.380 | 1013.100 | 1031.540 | 1027.540 | 1036.900 |
| 5000 ms sample latency (ms) | 5013.150 | 5016.590 | 5013.680 | 5031.180 | 5031.410 | 5038.650 |

## Capability Differences

- GPU capability flags: [True, True, True, False, False, False].
- Linux exposes task-attributable GPU capability on the current host; macOS reports `unsupported` without a trustworthy process mapping.
- Linux restart identity uses `/proc` process start time; the current macOS token has one-second resolution.
- T12 currently verifies a deliberate boundary: daemon restart does not restore terminal process, PTY, screen, metadata, or input.
- Manual T03/T04/T08/T11/T12 review records are platform evidence, not language-specific runtime scores.

## Missing Evidence

- No matrix case is missing from Rust, Go, or C++ on either platform.

## Interpretation Limits

- Results come from dirty worktrees until the accepted implementation is committed and regenerated.
- Single-host measurements are feasibility data, not production benchmarks.
- `blocked` means manual review or a deliberately unclaimed recovery capability remains; it is not a pass.
- Distribution version is recorded as environment metadata, not a target restriction.
