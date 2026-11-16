# QWE

Quick Window Environment is a lightweight, durable developer task runtime
built around terminal sessions, process groups, task history, and explicit
extension boundaries.

Fedora Linux x86_64 is the supported release target. Ubuntu 22.04/24.04 and
Debian 12 x86_64 are active development and sandbox-qualification targets.
macOS ARM64 remains local/experimental.

## Install and Use

- [Install QWE](INSTALL.md): download the published Linux V0 package or build
  it from source, then start the systemd user service.
- [Usage](USAGE.md): Sessions, Attach/detach, agent execution and capabilities,
  Leases, resources, credentials, and teardown.
- [QWE 0.2.2 notes](docs/releases/v0.2.2.md): current `dev` release candidate
  with Session layout, Agent Execution, Linux sandboxing, and automated
  versioned publishing.
- [QWE 0.2.1 notes](docs/releases/v0.2.1.md): superseded pre-publication
  candidate.
- [QWE 0.2.0 notes](docs/releases/v0.2.0.md): superseded pre-publication
  candidate.
- [QWE 0.1.1 notes](docs/releases/v0.1.1.md): historical Stage 6 release
  candidate; publication and pilot gates remain external.
- [Release](docs/release.md): supported platforms, package contract, and known
  limitations.

Stable releases live on the GitHub `main` branch and are tagged. The `dev`
branch is the integration branch for the next release.

## Current Phase

Stages 0-8 and the accepted first Agent Execution delivery are complete.
Multi-Pane layout and resource governance pass their supported-platform gates.
Stages 9.1-9.5 provide `exec`, stable command handles, bounded replay,
qwed-restart recovery, explicit worker-loss states, Principal capabilities,
revocation, descriptor credentials, redaction, read-only observation, and
merged PTY output.
See [ADR 0032](docs/adr/0032-agent-execution-model.md) and
[Agent Execution Model](docs/agent-execution-model.md).

[ADR 0033](docs/adr/0033-bubblewrap-sandbox-adapter.md) and
[ADR 0034](docs/adr/0034-sandboxed-pane-and-pty.md) implement the Linux
Bubblewrap sandbox for `AGENT_EXEC` and long-lived sandboxed Panes. The current
x86_64 matrix includes Fedora SELinux, Ubuntu AppArmor, and Debian targets.
See [Experiment 0051](docs/experiments/0051-sandbox-pane-poc-gates.md) for the
current evidence and fail-closed boundaries.

macOS binary distribution is deferred post-V0 under ADR 0020 and limited to
explicitly marked local self-signed packages under ADR 0019. aarch64,
openEuler, Kylin, and Rocky are not supported or scheduled. Other sandbox
backends, Mesh, plugins, and remote scheduling remain out of scope until
separately accepted.

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
