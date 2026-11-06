# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

Fedora Linux x86_64 is the supported V0 target. macOS ARM64 remains
local/experimental.

## Install and Use

- [Install QWE](INSTALL.md): download the published Linux V0 package or build
  it from source, then start the systemd user service.
- [Usage](USAGE.md): Sessions, Attach/detach, input, Leases, resources,
  credentials, and teardown.
- [QWE 0.1.1 notes](docs/releases/v0.1.1.md): current Linux x86_64 release
  notes; artifact publication remains a Stage 6 gate.
- [Release](docs/release.md): supported platforms, package contract, and known
  limitations.

Stable releases live on the GitHub `main` branch and are tagged. The `dev`
branch is the integration branch for the next release.

## Current Phase

Stages 0-5 are complete for the Linux-first V0 release. Stage 6 is in progress:
Linux V0 adoption and stabilization. Completed work includes hosted core CI and
the dedicated Fedora release-gate workflow. Clean-source publication, the real
Linux pilot, and the first `v0.1.x` patch release remain open.

Stage 7 is planned around Linux cgroup v2 resource governance for long-running
Sessions. It extends QWE's existing `qwed`/PTY model and does not introduce a
Docker runtime. See [Linux Resource Governance](docs/stage7-resource-governance.md).

macOS binary distribution is deferred post-V0 under ADR 0020 and limited to
explicitly marked local self-signed packages under ADR 0019. Fedora Linux
x86_64 remains the only supported V0 target. New Mesh, sandbox, plugin, and
remote-scheduling features remain out of scope until a separate accepted plan
and ADR.

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

Remote repositories:

- GitHub: `git@github.com:ripplewizard/qwe.git`
- Gitee: `git@gitee.com:uranus-yy/bianca.git`

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
just container-smoke
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
