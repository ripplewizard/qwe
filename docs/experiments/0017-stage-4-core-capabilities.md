# Experiment 0017: Stage 4 Core Capabilities

Status: Core and service-lifecycle slices passed. The acceptance items below
were subsequently closed by Experiment 0020.
Date: 2026-11-05
Environment: macOS 26.5.1 arm64 and Fedora Linux 35 x86_64, Rust 1.99.1

The initial per-process resource model in this experiment is superseded by
[Experiment 0018](0018-pane-resource-enforcement.md). Credential, security,
service, and teardown evidence remains valid.

## Scope

This experiment starts the Stage 4 implementation from `dev` and validates:

- Private credential loading and final-child descriptor delivery.
- Secret redaction from argv, environment, terminal output, status, and SQLite.
- Pane CPU backstop and explicit unsupported capabilities.
- Malformed/oversized client frames and oversized control input.
- Forced Session, batch Task, and server teardown.
- systemd and launchd manifest structure.
- Managed launchd/systemd crash restart, worker adoption, and stop cleanup.

It does not claim binary-replacement upgrade, signed Keychain ACL, macOS memory
enforcement, or real cross-UID installation acceptance.

## Commands

```sh
just service-config-check
just stage4-check
cargo test --workspace
```

## Observed Results

```json
{"case":"SVC-001","systemd_unit":"pass","launchd_plist":"pass","credential_boundary_documented":"pass"}
```

```json
{"case":"SVC-002/SVC-003","service_manager":"LaunchdService/SystemdService","worker_survived_qwed_sigkill":"pass","qwed_restart_adopted_worker":"pass","graceful_stop_cleaned_worker":"pass","session_closed":"pass","task_stopped":"pass"}
```

```json
{"case":"CRED-001/RES-001/SEC-001/KILL-001","credential_descriptor":"pass","credential_redaction":"pass","resource_capability":"pass","malformed_frame":"pass","oversized_input":"pass","session_teardown":"pass","batch_task_teardown":"pass","server_teardown":"pass"}
```

`plutil -lint` accepts the launchd agent. The Stage 4 probe uses real qwed,
worker, PTY, child processes, SQLite, and control sockets. The credential probe
reads a private file into qwed memory, transfers it through the private broker,
and confirms the PTY child receives descriptor 3. Its rendered screen reports
only the byte count and environment-leak count, never the secret.

The same `just stage4-check` command passes in a temporary Fedora Linux x86_64
checkout. Linux additionally reports `RLIMIT_AS` as supported and the probe
confirms a 256 MiB address-space policy terminates a 512 MiB allocation.

## Issues Found During the Slice

1. `portable-pty` closes every inherited descriptor above 2 after fork. The
   initial direct FD handoff therefore produced a bad descriptor in the final
   shell. The implementation now uses a one-shot owner-only broker socket and
   remaps it to descriptor 3 inside the internal launcher.
2. Rust `Command::exec` dropped the descriptor-inheritance property in the
   launcher. The launcher now uses `execvp` after an explicit close-on-exec
   check and retains the socket object.
3. A missing credential-provider root panicked during qwed startup. Provider
   construction now keeps absent configuration as an unconfigured backend and
   fails credential requests with `credential_unavailable`.

No secret bytes appeared in test evidence.

## Open Acceptance

- Launch a real cross-UID client in the installed socket layout.
- Validate public Developer ID trust and signed distribution. Local signed
  Keychain ACL and locked/unlocked behavior pass in Experiment 0019.

The cross-UID item is closed by
[Experiment 0020](0020-stage-4-closure.md) using an isolated private runtime
layout. Public Developer ID trust remains Stage 5 release packaging.
