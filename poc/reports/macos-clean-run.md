# macOS Clean POC Run

Status: passed
Date: 2026-11-03

## Provenance

- Source revision: `46b5092`
- Platform: macOS 26.5.1, arm64, 12 cores, 18 GB RAM
- Implementations: Rust 1.99.1, Go 1.27.3, Apple Clang 17
- Command: `just test-macos`

## Coverage

- 43 matrix cases and 129 expected latest macOS cells.
- 368 structured result files were produced across repeated preflight runs.
- Status distribution: 335 `pass`, 33 `blocked`, 0 `fail`.
- macOS-only schema and evidence validation: pass, 129/129 expected cells.

The blocked cells are T03/T04/T08/T11/T12 for each language. They remain blocked
until independent human sign-off, except T12 which preserves the original POC's
negative terminal-recovery result.

## Cross-Platform Result

The regenerated cross-platform report has no missing case for Rust, Go, or C++
on Linux and macOS. XP01 is available for review, while the manual cases remain
visibly blocked.

Raw PTY logs, terminal snapshots, and runtime databases remain local evidence
and are intentionally excluded from Git.
