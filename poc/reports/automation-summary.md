# Automated Test Summary

Status: machine-checkable POC matrix complete; manual sign-off and clean-source
regeneration remain.

## Platforms

- Linux x86_64: primary target, Fedora Linux 41 observed as environment metadata.
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
macOS ARM64. The latest 258 structured result documents pass
`poc/test-spec/result.schema.json` and all referenced evidence paths exist.

Automated domains passing for every implementation on both platforms:

- T01/T02 and T05–T07/T09/T10.
- P01–P12.
- D01–D08, including isolated disk-full injection.
- RM01–RM05.
- DX01–DX03.

Manual preflight domains T03/T04/T08/T11 are recorded but remain `blocked`
until independent human sign-off. T12 is `blocked` because the completed
preflight proves that terminal process, PTY, session metadata, screen state,
and input are not recovered after daemon restart.

## Review Outputs

- Comparison: `poc/reports/comparison.md`.
- Decision: `poc/reports/decision.md`.
- XP01: `poc/reports/platform-comparison.md`.
- EX01: `poc/reports/extension-review.md`.
- MC01: `poc/reports/dependency-review.md`.
- Remaining prerequisites: `poc/reports/remaining-validation.md`.

## Current Recommendation

Rust is the provisional recommendation, with Go as fallback and C++ not
preferred for a new core runtime. The recommendation is conditional on manual
sign-off and regeneration from a clean source commit.

## Known Gaps

- T12 requires a durable terminal host or explicit terminal-loss semantics.
- T08 replays the full raw PTY stream; compact VT-state attach is not implemented.
- Session leases are not persisted through daemon restart.
- IPC has request IDs and stable errors but no protocol version negotiation.
- Linux GPU discovery depends on `nvidia-smi`; unsupported capability is
  reported honestly.
- The current evidence revision is dirty and therefore not final scoring data.
