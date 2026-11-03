# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

## Current Phase

The Rust implementation foundation is active. It follows the accepted
architecture in
[`docs/qwe-design-baseline-v0.4.md`](docs/qwe-design-baseline-v0.4.md).

The process and crate boundaries are described in
[`docs/qwe-technical-architecture.md`](docs/qwe-technical-architecture.md).

## Repository Layout

- `docs/`: product, architecture, ADR, and POC documents
- `crates/`: production Rust libraries
- `apps/`: CLI, daemon, and durable host binaries
- `poc/`: cross-language POC harness and historical evidence

## Rust Commands

```sh
just check-qwe
just build-qwe
just test-qwe
just t12-host-smoke
just resilience-smoke
```

The Rust workspace requires the stable toolchain pinned in
`rust-toolchain.toml`.
