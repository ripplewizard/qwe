# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

Fedora Linux x86_64 is the supported V0 target. macOS ARM64 remains
local/experimental.

## Install and Use

- [Install QWE](INSTALL.md): download the published Linux V0 package or build
  it from source, then start the systemd user service.
- [Usage](USAGE.md): Sessions, Attach/detach, agent execution and capabilities,
  Leases, resources, credentials, and teardown.
- [QWE 0.1.1 notes](docs/releases/v0.1.1.md): current Linux x86_64 release
  candidate; publication remains pending
  notes; artifact publication remains a Stage 6 gate.
- [Release](docs/release.md): supported platforms, package contract, and known
  limitations.

Stable releases live on the GitHub `main` branch and are tagged. The `dev`
branch is the integration branch for the next release.

## Current Phase

Stages 0-5 and the Linux V0 release candidate are complete. Stages 7 and 8 pass
their resource-governance and Session-layout qualification gates.

Stage 6.5 accepted the shared prerequisites for one worker per Pane,
Pane-first ResourceScope, Pane-targeted channels, and the v10-to-v11 migration.
Stages 9.1-9.5 complete the accepted first Agent Execution Model delivery with
`exec`, stable command handles, bounded event replay, qwed-restart recovery,
explicit worker-loss states, Principal-bound capabilities, and administrative,
freeze, and terminate revocation, descriptor credentials, streaming redaction,
read-only observer push, merged PTY output, and interactive-shell foreground
`idle`.
See [ADR 0032](docs/adr/0032-agent-execution-model.md) and
[Agent Execution Model](docs/agent-execution-model.md).

[ADR 0033](docs/adr/0033-bubblewrap-sandbox-adapter.md) accepts Bubblewrap as
the first Linux sandbox backend. Core FD3, cgroup, structured PID, interactive
PTY, and parent-death probes pass. The capability-controlled non-PTY and PTY
`AGENT_EXEC` slice passes real qwed/worker integration tests, including
`TIOCSTI` denial, FD3 credentials, foreground idle, and process-tree cleanup.
The full probe also passes on Ubuntu 22.04 and Debian 12 real VMs; Ubuntu 24.04
fails closed under its tested AppArmor/user-namespace policy.

macOS binary distribution is deferred post-V0 under ADR 0020 and limited to
explicitly marked local self-signed packages under ADR 0019. Fedora Linux
x86_64 remains the only supported V0 target. Other sandbox backends, Mesh,
plugins, and remote scheduling remain out of scope until separately accepted.

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
