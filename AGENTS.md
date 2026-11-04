# Repository Guidelines

QWE (Quick Window Environment) is in the Rust implementation phase on `dev`.
Documents
in [`docs/`](docs/) are authoritative for the product baseline, production
architecture, POC evidence, and extension boundaries.

## Project Structure & Module Organization

- `docs/qwe-design-baseline-v0.5.md` defines the active implementation
  baseline. v0.4 is retained as history.
- `docs/qwe-technical-architecture.md` defines process and crate boundaries.
- `docs/ipc-protocol.md` defines qwed, Host, and worker authority and messages.
- `docs/development-readiness.md` defines the implementation start gate and
  current gaps.
- `docs/development-roadmap.md` defines implementation milestones.
- `docs/testing-strategy.md` defines test layers and acceptance criteria.
- `docs/development-workflow.md` defines branch and change flow.
- `docs/qwe-technical-poc-specification-v0.1.md` defines the POC plan.
- `docs/qwe-extension-architecture-notes-v0.1.md` records extension boundaries.
- `Justfile` orchestrates both POC regression and the Rust workspace.
- `crates/` and `apps/` contain the production Rust workspace.
- The active POC evidence worktree is `/Users/huangjian/project/uranus/bianca-poc`
  on branch `poc`; its `poc/{rust,go,cpp}/` contains equivalent POC
  implementations. Do not create POC run artifacts in the `dev` worktree.

## Build, Test, and Development Commands

```sh
just doctor              # Check environment and toolchains
just build all           # Build Rust, Go, and C++
just verify-skeleton     # Verify all POC skeletons
just verify-fixtures     # Verify shared test workloads
just test-macos          # Run the full macOS suite
just test-linux          # Run the full Linux suite
just test-linux-remote   # Run on istoria and pull Linux evidence
just check-qwe           # Format, lint, and test the Rust workspace
just build-qwe           # Build production Rust binaries
just resilience-smoke    # Verify recovery, persistence, and concurrent attach
just review-results      # Regenerate dependency/extension/platform reviews
just verify-linux-results # Validate the latest Linux matrix and evidence
just verify-macos-results # Validate the latest macOS matrix and evidence
```

## Branch Workflow

- `poc` is the unified, read-only POC evidence branch.
- `dev` is the production integration branch.
- Create feature work from `dev` using `codex/<topic>` branches.
- Historical `master`, `codex/poc-session-lease`, and
  `codex/rust-core-spikes` are POC lineage, not active development branches.

## Coding Style & Naming Conventions

Use UTF-8 Markdown with clear headings and language-tagged code fences.
Preserve established design identifiers such as `D###` and `PD###`. Name new
documents by topic, for example `docs/ipc-protocol.md`.

Use canonical Rust style. POC implementations remain reference evidence; do
not copy POC code directly into production crates.

## Testing Guidelines

Follow the unified test domains and acceptance criteria in
`poc/test-spec/matrix.md`. Mark tests with stable case IDs (`T05`, `P11`,
`D01`, etc.). Store structured evidence using
`poc/test-spec/result.schema.json`.

There is no coverage threshold yet. Tests must be deterministic, preserve
relevant output, and record environment, commit, and pass/fail results.
Manual POC sign-off is pending but does not block `dev` implementation.

## Commit & Pull Request Guidelines

Use Conventional Commits, for example:

```text
docs(baseline): clarify daemon recovery
feat(poc): add terminal resize test
```

PRs should explain the change, link the relevant issue, list validation
performed, and update design documentation when behavior or decisions change.
Include terminal screenshots for user-visible TUI changes.

## Scope & Security

Implement P0 behavior first; treat multi-node execution, sandboxing, credential
brokering, and other listed future features as out of scope until an explicit
design decision enables them. Report unsupported capabilities explicitly.
Never fake POC capabilities, system behavior, or test success.
Never commit credentials, tokens, SQLite state, terminal captures, or generated
runtime data.
