# Automated Test Summary

Status: automated POC matrix complete on Linux and macOS; independent manual
sign-off remains.

## Platforms

- Linux x86_64: primary target, Fedora Linux 35 observed as environment metadata.
- macOS ARM64: secondary comparison and preflight.

## Executed Suites

- `just fmt`
- `just lint`
- `just verify-skeleton`
- `just verify-fixtures`
- `just terminal-smoke`
- `just runtime-smoke`
- `just lease-smoke all`
- `just attach-smoke all`
- `just terminal-interaction-smoke all`
- `just timeout-smoke all`
- `just restart-smoke all`
- `just upgrade-smoke all`
- `just process-acceptance-smoke all`
- `just resource-smoke all`
- `just daemon-depth-smoke all`
- `just developer-experience-smoke all`
- `just test-macos`
- `just test-linux`
- `just review-results`
- `just verify-results`

## Coverage

All 43 matrix cases are represented for Rust, Go, and C++ on Linux x86_64 and
macOS ARM64. Linux evidence comes from clean revision `b42e2f9`; macOS evidence
comes from clean revision `46b5092`. Platform-specific verification passes for
129 expected cells on each platform. The cross-platform XP01 regenerated with
no missing platform/language/case cell.

Automated domains passing for every implementation on both platforms:

- T01/T02 and T05–T07/T09/T10.
- P01–P12.
- D01–D08, including isolated disk-full injection.
- RM01–RM05.
- DX01–DX03.

Manual preflight domains T03/T04/T08/T11 are recorded but remain `blocked`
until independent human sign-off. T12 remains `blocked` for the original POC
because terminal process, PTY, session metadata, screen state, and input are
not recovered after daemon restart. The production Rust architecture resolves
this separately in Experiment 0002.

## Review Outputs

- Comparison: `poc/reports/comparison.md`.
- Decision: `poc/reports/decision.md`.
- XP01: `poc/reports/platform-comparison.md`.
- EX01: `poc/reports/extension-review.md`.
- MC01: `poc/reports/dependency-review.md`.
- Remaining prerequisites: `poc/reports/remaining-validation.md`.

## Current Recommendation

Rust was selected for implementation, with Go as fallback and C++ not preferred
for a new core runtime. Automated evidence is complete on both platforms;
independent manual sign-off remains an audit task and does not block
implementation.

## Known Gaps

- POC T12 remains a deliberate negative result; production Experiment 0002
  validates durable terminal ownership in Rust.
- T08 replays the full raw PTY stream; compact VT-state attach is not implemented.
- Session leases are not persisted through daemon restart.
- IPC has request IDs and stable errors but no protocol version negotiation.
- Linux GPU discovery depends on `nvidia-smi`; unsupported capability is
  reported honestly.
- Manual T03/T04/T08/T11/T12 sign-off is still pending.
