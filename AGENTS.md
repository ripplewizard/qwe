# Repository Guidelines

QWE (Quick Window Environment) is in its design and technology-selection
phase. The documents in [`docs/`](docs/) are authoritative for the product
baseline, Rust/Go/C++ POC, and extension boundaries.

## Project Structure & Module Organization

- `docs/qwe-design-baseline-v0.2.md` defines the current product baseline.
- `docs/qwe-technical-poc-specification-v0.1.md` defines the POC plan.
- `docs/qwe-extension-architecture-notes-v0.1.md` records extension boundaries.
- `Justfile` is the only POC orchestration entry point.
- `poc/{rust,go,cpp}/` contains equivalent POC implementations; shared inputs,
  specifications, evidence, and reports stay outside language directories.
- Production source must wait for the recorded language decision.

Keep POC behavior comparable and identify reused components and owned
responsibilities in each implementation.

## Build, Test, and Development Commands

```sh
just doctor              # Check environment and toolchains
just build all           # Build Rust, Go, and C++
just verify-skeleton     # Verify all POC skeletons
just verify-fixtures     # Verify shared test workloads
just test-macos          # Run macOS-specific terminal tests
just test-linux          # Run Linux-specific tests on Linux
```

## Coding Style & Naming Conventions

Use UTF-8 Markdown with descriptive headings and language-tagged code fences.
Preserve established design identifiers such as `D###` and `PD###`. Name new
documents by topic, for example `docs/ipc-protocol.md`.

For future POC code, use each language's canonical formatter and naming style:
`snake_case` for Rust modules/functions and C++ functions, `PascalCase` for
types, and Go `MixedCaps`. Do not mix implementations or copy source between
language directories.

## Testing Guidelines

Follow the unified test domains and acceptance criteria in
`poc/test-spec/matrix.md`. Mark tests with stable case IDs (`T05`, `P11`,
`D01`, etc.). Store structured evidence using
`poc/test-spec/result.schema.json`.

There is no coverage threshold yet. Tests must be deterministic, preserve
relevant output, and record environment, commit, and pass/fail results.

## Commit & Pull Request Guidelines

The repository has no commit history, so no convention is established. Use
Conventional Commits from the first commit, for example:

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
