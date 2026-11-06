# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

## Current Phase

Active development starts on the `dev` branch. Stages 0-4 are complete;
Stage 5 release-candidate work is in progress. The current slice adds
checksummed native release packages, temporary-prefix installation,
in-place upgrade acceptance, release-mode Attach/soak benchmarks, and a
separate Developer ID notarization gate. Automated POC evidence is complete on
Linux and macOS; final manual audit disposition remains Stage 5 work.
Implementation follows
[`docs/design-baseline.md`](docs/design-baseline.md).

The process and crate boundaries are described in
[`docs/technical-architecture.md`](docs/technical-architecture.md).
Host authority and field-level IPC are in
[`docs/ipc-protocol.md`](docs/ipc-protocol.md).
See [`docs/development-readiness.md`](docs/development-readiness.md) for known
gaps and the implementation start gate.

## Repository Layout

- `docs/`: product, architecture, ADR, and POC documents
- `crates/`: production Rust libraries
- `apps/`: CLI, daemon, and durable host binaries
- `../bianca-poc/poc/`: external cross-language POC harness and evidence

The working POC evidence checkout is
`/Users/huangjian/project/uranus/bianca-poc` on branch `poc`. The `dev`
worktree is reserved for production development and ignores generated POC
results.

## Rust Commands

```sh
just check-qwe
just protocol-check
just stage0-check
just stage1-regression-smoke
just build-qwe
just test-qwe
just t12-host-smoke
just resilience-smoke
just stage4-check
just release-gate
```

Protocol schema validation uses
`scripts/requirements-protocol.txt`; `just doctor` reports whether the Python
`jsonschema` package is available.

## POC Audit Commands

```sh
just test-macos
just test-linux
just review-results
just platform-comparison
just verify-linux-results
just verify-macos-results
```

Use `poc` for historical evidence and `dev` for new production work.

The Rust workspace requires the stable toolchain pinned in
`rust-toolchain.toml`.
