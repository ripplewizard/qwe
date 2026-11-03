# Linux Clean POC Run

Status: passed
Date: 2026-11-03

## Provenance

- Source revision: `b42e2f9`
- Review provenance revision: `4fdc75d`
- Fedora evidence commit: `744961f`
- Fedora branch: `codex/qwe-rust-foundation` on `istoria`
- Platform: Fedora Linux 35, kernel 5.14.10, x86_64, 12 cores, 16 GB RAM
- Implementations: Rust 1.99.1, Go 1.27.3, GCC 11.2.1
- Command: `just test-linux`

## Coverage

- 43 matrix cases, 3 languages, 129 expected Linux latest cells.
- 159 structured result files generated from the clean revision, including
  repeated runs where the matrix intentionally records multiple executions.
- `just verify-linux-results`: pass.
- 141 result files report `pass`, 15 report `blocked`, and 3 report `fail`.

The blocked results are T03/T04/T08/T11/T12 for each language. The failed
results are XP01 for each language because macOS data has not yet been
regenerated; Linux matrix coverage itself is complete.

## Interpretation

Automated terminal, process, daemon, resource, and developer-experience domains
pass on Linux. The original POC T12 terminal-loss boundary remains explicit.
T03/T04/T08/T11 still require independent human sign-off before being promoted
from `blocked`.

Raw PTY logs and runtime databases remain local evidence because repository
policy prohibits committing terminal captures and runtime state. Structured
JSON results, manual observations, reports, and their source revisions are
committed.
