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
- [QWE 0.2.7 notes](docs/releases/v0.2.7.md): current Linux x86_64 release
  published from the protected `main` release flow.
- [QWE 0.2.6 notes](docs/releases/v0.2.6.md): previous published release.
- [QWE 0.2.5 notes](docs/releases/v0.2.5.md): superseded before publication.
- [Release](docs/release.md): supported platforms, package contract, and known
  limitations.

Stable releases live on the GitHub `main` branch and are tagged. The `dev`
branch is the integration branch for the next release.

## Quick Start

### 1. Install

Download the current Linux x86_64 archive and checksum from
[GitHub Releases](https://github.com/ripplewizard/qwe/releases), verify it, then
run:

```sh
tar -xzf qwe-0.2.7-linux-x86_64.tar.gz
cd qwe-0.2.7-linux-x86_64
./install.py
```

For source builds, Ubuntu AppArmor setup, upgrades, and uninstall instructions,
see [INSTALL.md](INSTALL.md).

### 2. Start the service

```sh
systemctl --user daemon-reload
systemctl --user enable --now qwe.service
export QWE_SOCKET="${XDG_RUNTIME_DIR}/qwe/qwe.sock"
qwe --socket "$QWE_SOCKET" ping
```

`ping` returns `{"daemon":"ok"}` when the service is ready.

### 3. Create and attach a Session

```sh
qwe --socket "$QWE_SOCKET" session create demo sh
qwe --socket "$QWE_SOCKET" session attach demo
```

Press `Ctrl-]` to detach without stopping the Session. Re-run `session attach`
to restore the terminal snapshot and live output.

### 4. Inspect and control work

```sh
qwe --socket "$QWE_SOCKET" status
qwe --socket "$QWE_SOCKET" session status demo
qwe --socket "$QWE_SOCKET" task history
qwe --socket "$QWE_SOCKET" session snapshot demo
```

`session snapshot` is diagnostic and bounded. It is not an unbounded terminal
replay.

### 5. Use multiple Panes

```sh
qwe --socket "$QWE_SOCKET" lease claim demo alice --ttl-ms 60000
qwe --socket "$QWE_SOCKET" --owner alice pane create demo sh --generation 1
qwe --socket "$QWE_SOCKET" session compose demo
```

Use `Ctrl-A n/p` to move focus, `Ctrl-A c` to create a Pane, `Ctrl-A x` to close
one, and `Ctrl-A [` for copy mode. See [USAGE.md](USAGE.md) for the full
compositor and floating-Pane controls.

### 6. Run an Agent command

Agent execution requires a Principal-bound capability for the target Pane:

```sh
qwe --socket "$QWE_SOCKET" security bootstrap
PRINCIPAL_ID="$(
  qwe --socket "$QWE_SOCKET" security principal-create local-root |
    jq -r .principal_id
)"
PANE_ID="$(
  qwe --socket "$QWE_SOCKET" session status demo |
    jq -r .pane_id
)"
qwe --socket "$QWE_SOCKET" security capability-issue "$PRINCIPAL_ID" \
  agent_exec --pane-id "$PANE_ID" \
  --output "$HOME/.config/qwe/agent.capability"
chmod 600 "$HOME/.config/qwe/agent.capability"
qwe --socket "$QWE_SOCKET" \
  --capability-file "$HOME/.config/qwe/agent.capability" \
  agent exec "$PANE_ID" /usr/bin/printf '%s\n' hello
```

Sandbox profiles, command replay/signals, PTY mode, credentials, and revocation
are documented in [USAGE.md](USAGE.md) and [Agent Execution Model](docs/agent-execution-model.md).

### 7. Stop and clean up

```sh
qwe --socket "$QWE_SOCKET" task stop TASK_ID
systemctl --user stop qwe.service
```

QWE preserves history until an explicit teardown. Review
[USAGE.md](USAGE.md) before deleting state under `~/.local/state/qwe`.

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
- Gitee source mirror only: `git@gitee.com:uranus-yy/bianca.git`

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
