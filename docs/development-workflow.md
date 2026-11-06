# Development Workflow

Status: Active
Date: 2026-11-04

## Branches

- `poc`: unified read-only archive of POC history and evidence.
- `dev`: integration branch for production development.
- `codex/<topic>`: short-lived implementation branches.

The POC branch is checked out separately at
`/Users/huangjian/project/uranus/bianca-poc`. Run POC-wide discovery and
evidence checks there. The `dev` worktree uses sparse checkout and excludes
`/poc/`; production smoke workloads live in `tests/fixtures/`.

Historical `master`, `codex/poc-session-lease`, and
`codex/rust-core-spikes` are ancestors of the unified POC history. Do not
continue feature work on them.

## Change Flow

1. Branch from `dev` as `codex/<topic>`.
2. Keep changes within one component or decision boundary.
3. Update tests and authoritative docs with behavior changes.
4. Run `just check-qwe`.
5. Run `just protocol-check` for protocol, state, or test-contract changes.
6. Run `just t12-host-smoke` and `just resilience-smoke` for ownership,
   persistence, concurrency, or protocol changes.
7. Run `just stage1-regression-smoke` for Session/Task lifecycle or
   reconciliation changes.
8. Update the state matrices and machine-readable protocol schema together for
   contract changes.
9. Merge back to `dev` only after validation.

## Continuous Integration

Every push to `dev` or `main` and every pull request runs
`.github/workflows/ci.yml` on hosted Linux and macOS runners. The workflow
checks formatting, Clippy, workspace tests, the canonical protocol schema, and
release builds.

The complete Fedora release gate is a separate manual workflow,
`.github/workflows/release-gate.yml`. It requires a runner registered with the
`self-hosted`, `linux`, `x64`, `fedora`, and `qwe-release` labels plus a live
systemd user manager. Hosted Ubuntu coverage never substitutes for Fedora
release qualification.

Release artifacts must be built from a clean commit and pass the Fedora
workflow before publication. Never upload an ignored local `dist/` archive
whose manifest reports a different source commit or `source_dirty: true`.

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
