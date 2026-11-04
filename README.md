# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

## Current Phase

Active development starts on the `dev` branch. Automated POC evidence is
complete on Linux and macOS; manual POC sign-off remains a non-blocking audit
task. Implementation follows
[`docs/qwe-design-baseline-v0.5.md`](docs/qwe-design-baseline-v0.5.md).

The process and crate boundaries are described in
[`docs/qwe-technical-architecture.md`](docs/qwe-technical-architecture.md).
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
just build-qwe
just test-qwe
just t12-host-smoke
just resilience-smoke
```

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
