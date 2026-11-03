# ADR 0005: Terminal State and Bounded Attach

Status: Accepted
Date: 2026-11-03

## Decision

V0 uses `portable-pty` for transport and `vt100` behind a QWE-owned
`TerminalState` interface. Replaceability matters more than adopting a larger
engine before requirements are measured.

Each session retains a bounded byte journal and a compact VT snapshot.

## Consequences

- Unbounded replay is prohibited.
- Slow clients cause backpressure, not unlimited memory growth.
- Truncated history is explicit.
- `alacritty_terminal` may replace `vt100` behind the interface if benchmarks
  or feature coverage justify it.
