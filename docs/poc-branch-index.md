# POC Branch Index

Status: Unified into `poc`

## Historical Branches

| Branch | Role | Tip |
|---|---|---|
| `master` | Initial cross-language scaffolding | `9abaff7` |
| `codex/poc-session-lease` | Session lease and runtime validation | `a9729f9` |
| `codex/rust-core-spikes` | Rust core spike history | `a9729f9` |
| `codex/qwe-rust-foundation` | Rust production foundation and platform evidence | before branch consolidation |

The branch histories are ancestors of the validated Rust foundation branch.
Creating `poc` at the final evidence commit therefore preserves all history
without inventing merge resolutions.

## Lookup Order

1. `poc/reports/decision.md` for the language decision.
2. `poc/reports/linux-clean-run.md` and
   `poc/reports/macos-clean-run.md` for clean platform runs.
3. `poc/test-spec/matrix.md` for case status and latest evidence.
4. `poc/reports/comparison.md` for weighted interpretation.
5. `poc/results/` for structured per-case results.

`dev` starts from the same validated documentation baseline but is the only
branch for new production code.
