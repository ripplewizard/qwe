# POC Index

The POC workspace is external to `dev`:

```text
/Users/huangjian/project/uranus/bianca-poc
```

It is a Git worktree on branch `poc` and contains historical specifications,
reports, structured results, and locally retained raw evidence.

## Primary POC Documents

All paths are relative to `/Users/huangjian/project/uranus/bianca-poc`:

- `docs/qwe-technical-poc-specification-v0.1.md`: POC design and scoring.
- `docs/qwe-design-baseline-v0.2.md`: historical product baseline.
- `poc/reports/decision.md`: Rust selection decision.
- `poc/reports/comparison.md`: language and platform comparison.
- `poc/reports/linux-clean-run.md`: clean Linux run.
- `poc/reports/macos-clean-run.md`: clean macOS run.
- `poc/test-spec/matrix.md`: latest test status and evidence.
- `poc/results/`: structured historical results.

## Historical Branches

| Branch | Role |
|---|---|
| `master` | Initial cross-language scaffolding |
| `codex/poc-session-lease` | Session lease and runtime validation |
| `codex/rust-core-spikes` | Rust core spike history |
| `codex/qwe-rust-foundation` | Rust foundation and platform evidence |
| `poc` | Unified read-only evidence branch |

Use Git log in the POC worktree for superseded reports and old revisions. Do
not reintroduce versioned current documents into `dev/docs`.
