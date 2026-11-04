# Development Workflow

Status: Active
Date: 2026-11-03

## Branches

- `poc`: unified read-only archive of POC history and evidence.
- `dev`: integration branch for production development.
- `codex/<topic>`: short-lived implementation branches.

The POC branch is checked out separately at
`/Users/huangjian/project/uranus/bianca-poc`. Run POC-wide discovery and
evidence checks there. Generated `poc/results/` files are ignored in `dev`.

Historical `master`, `codex/poc-session-lease`, and
`codex/rust-core-spikes` are ancestors of the unified POC history. Do not
continue feature work on them.

## Change Flow

1. Branch from `dev` as `codex/<topic>`.
2. Keep changes within one component or decision boundary.
3. Update tests and authoritative docs with behavior changes.
4. Run `just check-qwe`.
5. Run `just t12-host-smoke` and `just resilience-smoke` for ownership,
   persistence, concurrency, or protocol changes.
6. Merge back to `dev` only after validation.

## Commit Rules

Use Conventional Commits. Examples:

```text
feat(host): add session resize
fix(storage): preserve lease generation on restart
docs(architecture): define VT snapshot fields
```

Generated POC evidence is committed only as structured or concise review data.
Never commit credentials, raw terminal captures, runtime SQLite files, or
environment dumps.
