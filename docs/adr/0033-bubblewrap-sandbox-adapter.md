# ADR 0033: Bubblewrap Sandbox Adapter

Status: Accepted
Date: 2026-11-15
Extends: [ADR 0021](0021-shared-prerequisites.md),
[ADR 0032](0032-agent-execution-model.md)
Implementation state: Core Bubblewrap contract PoC complete; production
Linux `AGENT_EXEC` slice implemented and locally qualified for pipe-split and
PTY commands, including qwed restart, worker loss, and Pane teardown. Broader
platform qualification remains open.

## Context

QWE agent execution is now complete through Stage 9.5. Commands run in the Pane
`workload` cgroup, PTY commands preserve foreground-idle semantics, and
credentials arrive on inherited FD3. Linux cgroup v2 remains resource
governance, not an isolation boundary.

Bubblewrap is selected as the first external sandbox backend because it:

- is unprivileged on supported Linux systems;
- requires no daemon or container image;
- creates mount, user, PID, network, IPC, and UTS namespaces;
- preserves the existing Pane cgroup and process-tree model;
- supports explicit filesystem bind policy;
- can retain FD3 for command credentials; and
- can expose structured startup and exit status.

Bubblewrap is a policy executor, not a complete sandbox. QWE remains
responsible for profiles, path policy, network policy, capability authorization,
credential exposure, lifecycle, and fail-closed behavior.

## Decision

### 1. External Linux-only backend

The first sandbox backend is an external `bwrap` executable. QWE does not
implement mount or namespace setup itself.

- `QWE_BWRAP_PATH` selects an explicit executable; otherwise QWE resolves
  `bwrap` from the worker PATH. The packaged systemd PATH includes
  `/opt/local/bin` before distro paths.
- The backend is Linux-only.
- macOS reports `unsupported_platform`.
- A requested sandbox never falls back to unsandboxed execution.
- Bubblewrap absence, incompatible flags, or disabled unprivileged user
  namespaces return `sandbox_backend_unavailable`.
- Capability issue inspects `bwrap --help` and fails with
  `bwrap_version_incompatible` before issuing a token if any production option
  used by the adapter is absent. This checks observable backend features rather
  than inferring compatibility from a version string alone.
- Profile fields that cannot be enforced return `sandbox_profile_unsupported`.

Ubuntu 24.04 deployments may keep
`kernel.apparmor_restrict_unprivileged_userns=1` and load the packaged
path-scoped AppArmor profile. The profile grants `userns` only to Bubblewrap at
the documented `/usr/bin`, `/usr/local/bin`, and `/opt/local/bin` paths.
Disabling the global restriction is not the approved QWE deployment path.

Backend failure reasons must use observable failed-stage evidence, not stderr
guessing; container context does not by itself prove seccomp blocked namespace
creation. See Experiment 0049.

Sandbox error bodies expose a stable `details.reason` and unstable bounded
`details.diagnostics`; `AGENT_EXEC` startup failures use the same shape in
`error_details`. Full diagnostics require a high-trust Principal; other
Principals receive reason, environment, failed stage, and target only.

### 2. Existing execution topology remains authoritative

The Pane worker remains the command owner and direct Bubblewrap parent:

```text
qwe-worker
  -> qwe-session-exec
       -> bwrap
            -> qwe-sandbox-init (when Landlock is available)
                 -> sandboxed command
```

The worker is already inside the Pane scope. Bubblewrap and all descendants
inherit the `workload` cgroup before the command is released. QWE does not use
`clone3(CLONE_INTO_CGROUP)`, post-spawn cgroup migration, or Bubblewrap-managed
cgroups.

Pane cgroup cleanup remains the final process-tree cleanup authority.
`--die-with-parent` is defense in depth, not a replacement for worker or scope
cleanup.

### 3. Sandbox profile v1

The first profile is versioned and deny-unknown-fields:

```text
SandboxProfileV1
  workspace: { path, mode: readonly | read_write }
  protected_paths: [path]
  network: isolated_loopback | inherit
  home: hidden | readonly
  tmp: private
  process_namespace: isolated | shared
  setuid: denied
  seccomp: none | strict
```

`workspace:readonly` is the default. `workspace:read_write` is explicit.
`protected_paths` are re-mounted read-only after the workspace bind. This is a
risk reduction, not a claim that a writable workspace is trusted by host-side
processes.

Bubblewrap's `--unshare-net` provides an isolated network namespace containing
an UP loopback interface on the tested Bubblewrap 0.5, 0.6.1, 0.8, and 0.9
versions. QWE calls this `isolated_loopback`; there is no separate
`network:none` promise in v1. Stage 10 verifies both a real `127.0.0.1`
listen/connect pair and the absence of a default route.

Setuid privilege gain is denied by Bubblewrap's no-new-privileges state.
Sandbox status reports `setuid_denied:true` and `no_new_privs:true`; Stage 10
verifies `NoNewPrivs: 1` inside `/proc/self/status` on all tested versions.

The optional `strict` named seccomp profile is a small compatibility-oriented
denylist, not a syscall allowlist and not a complete syscall sandbox. It rejects
`ptrace`, `bpf`, `keyctl`, `add_key`, `request_key`, and `userfaultfd` with
`EPERM`, and `io_uring_setup` with `ENOSYS` so runtimes can detect an
unsupported facility through their normal fallback path. All sandbox profiles,
including `seccomp:none`, retain the adapter's `TIOCSTI` denial with `EPERM`.
Unknown named profiles fail capability issue with
`sandbox_profile_unsupported`.

Seccomp audit identifiers are content-addressed. The effective cBPF program is
hashed with SHA-256 and returned as `sandbox_audit.seccomp_profile_hash`;
the stable profile name is descriptive metadata, while the hash identifies the
bytes passed to Bubblewrap.

### 4. Capability controls profile choice

The optional profile on an `agent_exec` capability is the enforced profile, not
a client-selectable menu.

- A capability with `sandbox_profile` always executes through Bubblewrap.
- A capability without a profile retains the explicit unsandboxed legacy path.
- A `sandbox_profile` supplied in a client `AGENT_EXEC` payload is rejected
  with `sandbox_profile_not_client_selectable`.
- Profile choice occurs when a management capability issues `agent_exec`; it is
  not an ambient client setting.

The first delivery stores one profile per capability. Multiple allowed profiles
or a request-time selector require a later compatible protocol extension.

### 5. Process identity and PTY

QWE invokes Bubblewrap with `--json-status-fd` and requires the structured
`child-pid` record before treating startup as successful. QWE does not parse
human-readable stderr for state.

The current `portable-pty` path establishes QWE's controlling terminal. The
core PoC showed:

- with `--new-session`, the PTY foreground group maps to outer Bubblewrap, but
  bash reports “no job control in this shell”;
- without `--new-session`, `stty size`, resize, SIGINT, SIGTSTP, `jobs`, and
  foreground-job interruption all work;
- without `--new-session`, the PTY foreground group differs from outer
  Bubblewrap and must be mapped separately; and
- structured `child-pid` identifies the actual command in the parent PID
  namespace and differs from the outer PID.

An interactive sandbox command distinguishes:

- wrapper PID used for command ownership and cgroup inspection;
- sandbox child PID reported by Bubblewrap; and
- `signal_pgid`, initialized from the child PID and refreshed from the current
  PTY foreground group for signals and idle.

`--new-session` is not used for interactive PTY sandbox commands. Disconnecting
the controlling terminal breaks shell job control. The production adapter
preserves the portable-pty controlling terminal and loads an internal cBPF
seccomp filter that rejects `ioctl(TIOCSTI)` with `EPERM`. The filter is adapter
hardening and does not change the user profile's `seccomp:none` meaning.

`portable-pty` closes inherited descriptors above stdio before exec. Sandbox
status and seccomp inputs are therefore connected by `qwe-session-exec` after
PTY setup: a mode-0600 status socket maps to FD4, and a mode-0600 compiled
filter file maps to FD5 before Bubblewrap starts.

### 6. Credentials

FD3 remains the only credential transport. QWE preserves FD3 through
Bubblewrap and keeps redaction outside the sandbox.

Sandboxing cannot guarantee credential confidentiality:

- `network:inherit` permits direct exfiltration;
- writable workspace permits a command to persist the secret itself; and
- output redaction does not inspect files written to the workspace.

Profiles carrying credentials must declare this limitation. Stronger credential
guarantees require `network:isolated_loopback`, an ephemeral workspace, or a
future broker design.

Stage 10 confirms both channels with direct FD3 tests: a host loopback listener
receives the credential under `network:inherit`, and a host-visible workspace
file retains it after command exit under `workspace:read_write`. The derived
risk flags are therefore auditable claims about capability composition, not
guarantees that every attempted transfer succeeds.

### 7. Landlock defense in depth

Landlock is optional defense in depth on Linux; Bubblewrap remains the only
primary sandbox backend and there is no Landlock-only mode. When the kernel
reports Landlock ABI 1 or newer, the adapter binds `qwe-sandbox-init` into the
Bubblewrap mount namespace. The init process enables `PR_SET_NO_NEW_PRIVS`,
applies an ABI-aware write-access ruleset, and then executes the requested
program.

The ruleset handles file writes, removals, and file/directory/socket/FIFO/device
creation. It allows those mutations only beneath the workspace when
`workspace:read_write`, private `/tmp`, and `/dev`. Read, directory listing, and
execute are intentionally not handled, preserving runtime and loader behavior.
ABI 2 adds cross-directory `REFER`, ABI 3 adds `TRUNCATE`, and ABI 5 adds
device `IOCTL` handling. Landlock ABI 4's TCP access rights are not handled
because `network:isolated_loopback` and Bubblewrap remain the network policy.

This is narrower than a complete filesystem sandbox:

- Bubblewrap remains authoritative for mount visibility and protected paths;
- Landlock path-beneath rules cannot express a write deny beneath an allowed
  writable workspace, so protected-path enforcement still depends on
  Bubblewrap;
- unhandled operations such as metadata changes remain outside this ruleset;
  and
- when Landlock is unavailable, the command still runs through mandatory
  Bubblewrap confinement but without this extra layer.

Capability audit metadata records the detected `landlock_abi` when available.
The environment variable `QWE_LANDLOCK_ABI` is set only after successful
initialization. A missing init binary when Landlock is otherwise available
fails command startup closed; it never switches to an unsandboxed path.
Capability issue validates the init binary in advance and returns
`sandbox_init_unavailable` before issuing a token if it is missing or not
executable.

### 8. Lifecycle synchronization

`--die-with-parent` depends on the thread that creates Bubblewrap remaining
alive through sandbox setup. The implementation must:

- spawn from a dedicated command lifetime thread;
- wait for the structured startup record before considering the command
  running; and
- keep the creator thread alive until the command reaches a terminal state.

The Pane cgroup and Pane teardown remain the final cleanup fallback.

### 9. First delivery

The first production slice applies to Linux `AGENT_EXEC` only:

- workspace read-only or read-write profiles;
- hidden or read-only home;
- private `/tmp`;
- isolated-loopback or inherited network;
- PID namespace isolation;
- FD3 credentials and existing redaction;
- pipe-split and PTY commands;
- fail-closed backend/profile errors.

Capability issue validates the profile, requires the workspace to be an
existing absolute directory, requires protected paths to be existing absolute
paths inside that workspace, and probes Bubblewrap before returning a token.
The probe is fresh on every capability issue; qwed does not cache a previous
success or failure. qwed's capability-issue lock serializes these probes.
Repairing AppArmor, namespace sysctls, or Bubblewrap installation is therefore
visible to the next issue request without restarting qwed or the Pane worker.
An already-issued capability is authorization, not a permanent health claim:
if the backend becomes unavailable later, the actual Bubblewrap spawn fails
closed.

The worker repeats capability-independent profile checks and requires the
structured startup record before reporting `running`.

Session and Pane startup-program sandboxing follows after the agent command
path is qualified. Additional seccomp profiles, Landlock-only fallback,
terminal/signal responsibilities for an init shim, external sandbox providers,
and untrusted multi-tenant isolation require separate decisions.

### 10. Parallel workstreams and dependencies

Two follow-on routes are deliberately separate:

1. **Sandboxed Pane startup**

   This remains a product decision. The current recommended shape is default
   unsandboxed Panes plus an explicit, capability-authorized sandboxed Pane
   mode; `SESSION_CREATE.sandbox_profile` is not accepted as ambient selection.
   Before acceptance, qualification must cover:

   - controlling-terminal, resize, SIGINT/SIGTSTP, and job control for a
     long-lived wrapped shell;
   - dedicated creator-thread lifetime and `--die-with-parent`;
   - qwed restart, Attach reconnect, worker loss, and Pane teardown;
   - whether an `AGENT_EXEC` sandbox nested inside a Pane sandbox can only
     tighten, never broaden, the effective policy; and
   - an explicit capability boundary for the sandboxed Pane operation.

   The current direct-Bubblewrap, no-terminal-shim conclusion applies only to
   `AGENT_EXEC`. `qwe-sandbox-init` currently applies Landlock and immediately
   execs; it does not establish a controlling terminal, forward signals, or
   synchronize window size. Those responsibilities remain unresolved for an
   interactive Pane shell.

2. **Backend hardening for `AGENT_EXEC`**

   Named seccomp profiles and Landlock defense-in-depth do not depend on the
   Pane product decision and can be developed independently. The current
   Landlock layer is attached inside mandatory Bubblewrap confinement. Any
   future Landlock-only mode must use a distinct backend type and must never be
   reported as a Bubblewrap sandbox.

## Validation

Core Rust contract probes live in
`apps/qwe-worker/tests/bubblewrap_contract.rs` and run with:

```sh
just bubblewrap-poc
```

The probes require Linux and `bwrap`; otherwise they skip. Current PoC results:

- FD3 reaches the target unchanged;
- Bubblewrap descendants inherit the QWE cgroup;
- `--json-status-fd` reports a host child PID distinct from outer Bubblewrap;
- `--die-with-parent` terminates the sandbox after a synchronized parent-thread
  exit;
- non-PTY commands preserve FD3/cgroup/startup/exit semantics;
- PTY resize, SIGINT, SIGTSTP, and shell job control pass without
  `--new-session`; and
- `--new-session` maps foreground pgrp to outer Bubblewrap but loses shell job
  control.
- `just sandbox-container-matrix` verifies Ubuntu 22.04, Ubuntu 24.04, and
  Debian 12 fail closed when Podman prevents Bubblewrap from establishing its
  nested mount/PID environment. This is a fail-closed deployment check, not a
  successful sandbox qualification inside containers.
- `just sandbox-vm-matrix` passes the complete Stage 10 probe on Ubuntu 22.04
  and Debian 12. Default Ubuntu 24.04 rejects user-ID mapping and qwed returns
  `sandbox_backend_unavailable`.
- `just sandbox-userns-disabled-vm-matrix` disables user namespaces globally on
  Ubuntu 22.04, Ubuntu 24.04, and Debian 12, verifies `userns_disabled`, runs
  an ordinary Pane command successfully in each VM, and restores sysctls.
- `just sandbox-selinux-vm` runs the complete Stage 10 probe on Fedora 44 with
  SELinux confirmed `Enforcing` before and after execution.
- `just sandbox-bubblewrap-oldest-vm-matrix` builds the pinned Bubblewrap
  0.5.0 source and passes the complete Stage 10 probe on Ubuntu 22.04, Ubuntu
  24.04 under the packaged AppArmor profile, and Debian 12.
- `just sandbox-apparmor-vm` loads the packaged path-scoped profile on Ubuntu
  24.04 while preserving `apparmor_restrict_unprivileged_userns=1`, verifies the
  expected profiles are loaded, and passes the complete Stage 10 probe.
- `just sandbox-systemd-properties` verifies an RW workspace under `/run/user`
  and `$HOME` with the packaged systemd hardening properties.
- `just sandbox-systemd-service-smoke` verifies sandboxed Agent execution,
  qwed crash recovery, binary replacement, and teardown in a transient user
  service using the packaged security properties.
- `just stage10-sandbox` verifies the `strict` named seccomp profile in both
  pipe-split and PTY commands, including denial return codes and the distinct
  seccomp content hash in capability audit metadata.
- `just landlock-poc` verifies Landlock ABI detection, RW/RO workspace write
  enforcement, denial outside allowed write roots, preservation of reads and
  private `/tmp`, and whichever ABI-specific rights the host kernel exposes.
- `just sandbox-landlock-vm` verifies ABI 4 directly on Ubuntu 24.04:
  cross-directory rename is denied through `REFER`, truncation is denied
  through `TRUNCATE`, while workspace and private `/tmp` writes remain usable.
- Stage 10 runs qwed from an isolated binary directory without the init helper
  and verifies capability issue fails closed with `sandbox_init_unavailable`.
- The same probe supplies a backend with a valid `--version` but an incomplete
  `--help` contract and verifies capability issue fails with
  `bwrap_version_incompatible` before issuing a token.

Remaining qualification before production:

- additional SELinux-enforcing distributions and user-namespace-disabled
  configurations beyond the current Fedora/Ubuntu/Debian coverage;
- additional named seccomp profiles and wider runtime compatibility coverage;
- broader network escape and filesystem escape probes;
- credential/file-exfiltration cases;
- interactive Agent PTY input/resize protocol, if that becomes a product
  requirement.

Stage 10.1-10.4 evidence is recorded in
[Experiment 0049](../experiments/0049-stage10-sandbox-integration.md).

## Consequences

- QWE gains a practical Linux sandbox without a daemon or registry/image
  dependency.
- Fail-closed semantics may reject usable environments where user namespaces
  are disabled.
- A writable workspace remains an intentional trust-transfer channel.
- Bubblewrap version and feature probing become part of startup and capability
  reporting.
- No production path may silently relabel cgroup resource limits as sandboxing.

## References

- [Bubblewrap](https://github.com/containers/bubblewrap)
- [Landlock: unprivileged access control](https://docs.kernel.org/userspace-api/landlock.html)
- [Feature Assessment: Sandbox tiers](../feature-assessment-agent-runtime-and-ux.md)
- [Agent Execution Model](../agent-execution-model.md)
