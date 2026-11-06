# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

## Current Phase

Active development starts on the `dev` branch. Stages 0-5 are complete for the
Linux-first V0 release. The release adds
checksummed native release packages, temporary-prefix installation,
in-place upgrade acceptance, release-mode Attach/soak benchmarks, and a
separate Developer ID notarization gate. macOS binary distribution is
deferred post-V0 under ADR 0020 and limited to explicitly marked local
self-signed packages under ADR 0019. Fedora Linux x86_64 is the supported V0
target; the final historical audit disposition and independent manual
acceptance are complete.
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
