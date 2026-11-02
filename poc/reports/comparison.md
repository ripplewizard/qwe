# POC Comparison and Weighted Scores

Status: provisional. All automated and machine-checkable domains are complete
on Linux x86_64 and macOS ARM64. Manual terminal observations are recorded, but
they still need independent human sign-off before this becomes final selection
evidence.

## Run Metadata

| Field | Value |
|---|---|
| Source commit | `9abaff7-dirty` |
| Linux environment | Fedora Linux 41, kernel 6.11.4, x86_64, 16 cores, 32 GB RAM |
| macOS environment | macOS 27.0.1, arm64, 20 cores, 64 GB RAM |
| Toolchains | Rust 1.99.1, Go 1.27.3, GCC 14.2.1, Apple Clang 21.0.0 |
| Terminal tools | Neovim 0.10.4/Linux and 0.12.6/macOS; htop 3.3.0/3.5.3 |
| Raw comparison | [`platform-comparison.md`](platform-comparison.md) |

The dirty source label is intentional: this checkout contains the accepted POC
implementation but has not yet been committed and rerun from a clean revision.

## Scoring Method

Scores use the required 1–5 scale. They are based on structured results,
dependency review, extension-boundary review, and the manual terminal evidence.
Performance numbers are secondary: a faster implementation cannot pass a failed
hard gate.

## Weighted Scores

| Domain | Weight | Rust | Go | C++ | Basis |
|---|---:|---:|---:|---:|---|
| Terminal Runtime | 25% | 4 | 3 | 3 | All pass PTY, VT, Unicode, resize, attach, htop, Neovim and job control. Rust has the healthiest parser/PTY pairing; Go's `vt10x` is stale; C++ carries a larger low-level libvterm integration burden. |
| Process Runtime | 20% | 4 | 4 | 4 | P01–P12 pass on both platforms, including groups, signals, PID reuse and restart history. |
| Daemon Architecture | 15% | 4 | 4 | 3 | D01–D08 including isolated disk-full pass. C++'s manual socket/thread/process ownership is the highest maintenance cost. T12 remains a shared terminal architecture gap. |
| Resource Monitoring | 10% | 4 | 3 | 3 | RM01–RM05 pass; Go adds cgo exposure and C++ requires more platform glue. Linux reports GPU capability, macOS correctly reports unsupported. |
| Developer Experience | 10% | 4 | 3 | 2 | Rust and Go provide concise generated-error and dependency workflows. Go's `vt10x` and cgo are liabilities. C++ builds fastest but requires the most low-level ownership and integration work. |
| Cross-platform | 5% | 4 | 4 | 3 | All build on Linux/macOS; C++ needs an external libvterm and more platform-specific linking. |
| Extension Architecture | 5% | 3 | 3 | 3 | Core object separation is directionally sound, but all lack durable terminal ownership, compact VT state and versioned IPC. |
| Maintenance Cost | 10% | 4 | 3 | 3 | Rust has the strongest active dependency set. `vt10x` is the weakest dependency signal; libvterm creates the largest adaptation surface. |
| **Total** | **100%** | **3.95** | **3.40** | **3.10** | Weighted arithmetic from the column above. |

## Hard Gates

| Gate | Rust | Go | C++ | Evidence |
|---|---|---|---|---|
| Basic terminal interaction | Pass* | Pass* | Pass* | T01–T11 automated checks; T03/T04/T08/T11 manual records |
| Reliable process-group control | Pass | Pass | Pass | P02/P03/P04 |
| Tasks survive daemon crash | Pass | Pass | Pass | P08 |
| Recoverable TaskInstance identity | Pass* | Pass* | Pass* | P09/P12; macOS start token is coarser |
| IPC and persistence errors explicit | Pass | Pass | Pass | D01/D04/D05/D06 |
| Dependency health acceptable | Pass | Pass with `vt10x` risk | Pass | MC01 |

`Pass*` means the technical check passed but final independent human terminal
sign-off is still pending.

## Additional Metrics

Linux is the primary column; macOS values are in
[`platform-comparison.md`](platform-comparison.md).

| Metric | Rust | Go | C++ | Notes |
|---|---:|---:|---:|---|
| Declared direct dependencies | 4 | 3 | 3 | Go uses three modules but one is a stale pseudo-version. |
| CLI probe median | 0.772 ms | 2.022 ms | 1.484 ms | Linux |
| IPC missing-status median | 0.428 ms | 0.177 ms | 0.411 ms | Linux |
| Concurrent-status median | 1.906 ms | 1.419 ms | 1.889 ms | 40 requests; Linux |
| Clean build | 35.904 s | 29.123 s | 3.260 s | Fresh target/build directories |
| 50k-line output | 252 ms | 252 ms | 250 ms | Same shared workload |
| Daemon RSS after 8 s | 4,872 KiB | 10,680 KiB | 5,664 KiB | Linux D08 |
| 100 ms sample latency | 114.71 ms | 116.53 ms | 118.01 ms | Linux RM03 |
| 1000 ms sample latency | 1012.47 ms | 1011.19 ms | 1014.76 ms | Linux RM03 |
| 5000 ms sample latency | 5010.90 ms | 5018.64 ms | 5012.94 ms | Linux RM03 |
| Implementation-time from zero | Not reconstructable | Not reconstructable | Not reconstructable | Must not be inferred retrospectively. |

## Terminal Framework Comparison

| Language | Current components | Assessment |
|---|---|---|
| Rust | `portable-pty` + `vt100` | Best current balance. PTY and VT roles are separated, dependency health is good, and structured Rust ownership fits daemon/session boundaries. Production should evaluate `alacritty_terminal` or a maintained equivalent for delta state. |
| Go | `creack/pty` + `vt10x` + `go-sqlite3` | PTY and daemon ergonomics are good. `vt10x` is the main technical liability; a production choice should replace or own that parser. |
| C++ | POSIX PTY + `libvterm` + SQLite | Capable and fastest to clean-build, but the most manual memory, threading, error and dependency integration. Strong only if C++ is required for organizational or ecosystem reasons. |

## Decision Inputs

1. All three roads are technically feasible for the current POC scope.
2. Rust is the provisional recommendation because it wins on dependency health
   and integration economy without losing any runtime hard gate.
3. Go is the fallback when goroutine/concurrency simplicity or Go delivery
   alignment outweighs the `vt10x` replacement cost.
4. C++ is not preferred for a new core runtime without a separate requirement
   that justifies its lower-level implementation cost.
5. The shared T12 limitation must be solved architecturally; it is not evidence
   that one of the three languages is inherently incapable.

## Remaining Unknowns

- Independent human sign-off for the recorded T03/T04/T08/T11 observations.
- Production terminal ownership outside the daemon and compact VT state.
- Versioned IPC and global identity for future Mesh support.
- Longer-running stability, GPU attribution on real multi-process workloads,
  and repeated performance runs from a clean simultaneous baseline.
