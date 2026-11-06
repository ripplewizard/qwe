# Experiment 0023: V0 Historical Evidence Audit

Status: Disposition complete; no historical result rewritten
Date: 2026-11-06

## Scope

This audit closes the historical POC/manual disposition required by Stage 5.
It does not alter raw evidence, promote blocked POC rows, or treat a production
Rust result as a retroactive result for the cross-language POC.

## T03/T04/T08/T11 Disposition

The historical POC matrix retains these rows as `blocked` for the original
cross-language implementations because independent human sign-off was not
recorded for that POC path.

The production Rust path has separate acceptance evidence:

- [Experiment 0010](0010-stage-2-manual-tui-signoff.md) records independent
  reviewer `hjian` and PASS results for T03/T04/T08/T11 on macOS ARM64 and
  Fedora Linux x86_64 at reviewed revision `c16a557`.
- [Experiment 0011](0011-kitty-keyboard-reset.md) records the subsequent Kitty
  keyboard-protocol reset fix and `ATTACH-015` regression.

Disposition: the historical POC rows remain blocked and archived. Production
Rust acceptance is established by its own signed report. Neither record
supersedes the other retroactively.

## T12 Disposition

The historical POC T12 result remains a negative architectural finding:
restarting the POC daemon did not restore the terminal process, PTY, screen,
metadata, or input.

The production architecture changed the ownership boundary and has separate
recovery evidence:

- [Experiment 0001](0001-t12-host-ownership.md) covers durable terminal
  ownership and bounded output.
- [Experiment 0009](0009-stages-1-3-audit.md) covers production T12,
  resilience, worker-loss, reconciliation, and leases on both platforms.
- [Experiment 0012](0012-stage-3-closure.md) records Stage 3 closure and keeps
  low-frequency Fedora timeout behavior visible as a watch item.

Disposition: the original POC T12 remains negative/blocked history. The
production Rust architecture passes its separate recovery gates. No document
claims that the old POC T12 test passed.

## Evidence Retention

Raw PTY logs, daemon logs, terminal captures, and SQLite files remain local as
required by repository policy. Only structured summaries and stable evidence
references are committed.

## Audit Result

No historical evidence was deleted, relabeled, or rewritten. The Stage 5
POC/manual audit disposition is complete, and no additional historical-evidence
blocker remains.
