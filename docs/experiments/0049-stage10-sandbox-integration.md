# Experiment 0049: Stage 10.1-10.4 Sandbox Integration

Date: 2026-11-15
Status: Pass for Linux `AGENT_EXEC` pipe-split and PTY commands, including
recovery and teardown

## Result

The capability-controlled Bubblewrap path now runs through the production
qwed, Pane worker, `qwe-session-exec`, and Bubblewrap chain. The probe verifies:

- a capability profile executes commands through Bubblewrap;
- a request payload cannot enable sandboxing on an unsandboxed capability;
- a request payload cannot replace or downgrade a capability profile;
- workspace read-only and read-write modes are enforced;
- `home:read_only` exposes HOME but rejects writes;
- `process_namespace:shared` makes Pane worker processes visible through
  `/proc`;
- `network:inherit` shares the host network namespace and can send an inherited
  FD3 credential to a host loopback listener;
- an existing protected path inside a writable workspace remains read-only;
- protected-path symlinks resolve inside the canonical workspace and remain
  read-only;
- host `$HOME` is absent and `/tmp` is private;
- host PIDs, `/etc/shadow`, `/sys`, and host environment values are absent;
- workspace symlinks to host paths outside the mount policy are inaccessible;
- isolated networking has no default route, keeps `lo` UP, and supports a
  local `127.0.0.1` listen/connect pair;
- `NoNewPrivs: 1` makes setuid privilege gain denial an observable guarantee;
- FD3 credentials and exact-match redaction remain intact;
- wrapper and structured child PIDs both inherit the Pane workload cgroup;
- sandboxed PTY commands preserve terminal size, FD3 credentials, redaction,
  foreground idle, termination, and cgroup ownership;
- a seccomp filter rejects `TIOCSTI` with `EPERM` while preserving normal PTY
  operation; and
- an unavailable Bubblewrap executable fails capability issue closed;
- qwed restart reconnects to the same worker, wrapper, child, profile, and
  events;
- forced Pane teardown removes the worker, wrapper, child, descendants, and
  cgroup; and
- worker loss marks commands `lost`, preserves the enforced profile, and
  removes the sandbox process tree.

The confirmed `network:inherit + FD3 credential` transfer is a known-risk
capability, not a redaction bug. Command status exposes structured
`sandbox.risk` flags and events include `sandbox_risk_observed`. These identify
network exfiltration and workspace persistence channels. Current risk events
and the base risk flags are persisted in SQLite, so they survive worker loss
and qwed restart. Event replay itself remains worker-memory bounded.

The probe is `tests/fixtures/stage10_sandbox_probe.py` and runs with:

```sh
just stage10-sandbox
```

## Rust Integration Findings

### Child reaping ownership

qwed's original `waitpid(-1, WNOHANG)` monitor reaped the qwed-local Bubblewrap
namespace probe child before the probe could call `try_wait`, producing
`ECHILD`. The probe now runs in the target Pane worker, where it also tests the
actual worker environment. qwed retains its generic subreaper loop so orphaned
Bubblewrap wrappers and descendants are reaped instead of remaining zombies.

When the worker is lost, qwed reconstructs `lost` status from durable command
metadata plus the creating capability. The returned profile remains visible,
while runtime child identity is unavailable.

### Startup descriptor and cgroup

The worker creates a non-CLOEXEC status pipe above descriptor 10, passes its
write end through `qwe-session-exec` to `bwrap --json-status-fd`, and does not
report the command as running until the first JSON record contains
`child-pid`. The outer `qwe-session-exec`/Bubblewrap PID is attached to the Pane
workload cgroup before the release handshake; descendants inherit it.

`portable-pty` unconditionally closes inherited descriptors above stdio before
exec. Pipe-split commands can inherit status and credential descriptors
directly. PTY commands instead let `qwe-session-exec` connect a mode-0600 status
socket to FD4 and open a mode-0600 compiled seccomp file on FD5 immediately
before executing Bubblewrap.

The status response reports wrapper PID, structured child PID, and current
signal process group separately. Commands are identified by the stable
`qwe-session-exec` wrapper, while PTY idle checks and signals use the current
foreground group with the structured child as fallback.

### Bubblewrap 0.5.0 compatibility

Bubblewrap 0.5.0 rejects the newer `--preserve-fds` option. FD3 still reaches
the target when `qwe-session-exec` clears close-on-exec before executing
Bubblewrap, matching Experiment 0048. The v1 adapter therefore does not depend
on `--preserve-fds`.

### Distribution library layout

The first real Ubuntu 22.04 and Debian 12 VM run exposed that the capability
probe mounted `/lib64` but not `/lib`. Both distributions need `/lib` for the
dynamic loader, while the full sandbox mount policy already included it. The
probe and Bubblewrap contract shell now bind `/lib` as well.

### Backend probe lifetime

The capability probe is intentionally fresh for every capability issue and
returns `fresh:true` from the worker. The implementation does not cache a
success or failure inside the Pane worker. This allows a corrected AppArmor
profile, namespace sysctl, or Bubblewrap installation to take effect on the
next issue request without restarting qwed or an existing worker.

qwed's existing capability-issue lock serializes issue requests inside one
daemon. A previously issued capability is not a continuing health assertion:
if the backend changes after issuance, the real command spawn fails closed.

On the Fedora 35 host, 20 repetitions of the two-process probe averaged about
3 ms per capability issue. The Stage 10 probe also removes the installed-backend
condition, issues a new capability against the same worker, and requires success
without restarting qwed or the worker.

### Loopback and setuid privilege boundary

The same probe was run across available Bubblewrap and distribution versions:

| Distribution | Bubblewrap | Context | `lo` flags | 127.0.0.1 connect | NoNewPrivs |
|---|---:|---|---:|---|---|
| Fedora 35 | 0.5.0 | unprivileged | `0x49` | pass | pass |
| Fedora 35 | 0.13.1 | compiled install at `/opt/local/bin/bwrap` | `0x49` | pass | pass |
| Ubuntu 22.04 | 0.6.1 | unprivileged | `0x49` | pass | pass |
| Debian 12 | 0.8.0 | unprivileged | `0x49` | pass | pass |
| Ubuntu 24.04 | 0.9.0 | unprivileged, AppArmor restriction temporarily disabled | `0x49` | pass | pass |

`0x49` contains both `IFF_UP` and `IFF_LOOPBACK`. Ubuntu 24.04 restored
`kernel.apparmor_restrict_unprivileged_userns=1` after the probe.
These are verified versions, not a guarantee for future Bubblewrap releases;
the loopback probe must run whenever the backend version changes.

The compiled 0.13.1 installation is selected explicitly through
`QWE_BWRAP_PATH` in Stage 10. The packaged systemd unit likewise puts
`/opt/local/bin` before distro paths, avoiding a mismatch between an interactive
shell and the qwed service.

### Backend failure classification

Bubblewrap stderr alone does not reliably distinguish user-namespace denial
from container restrictions. The observed evidence separates them:

| Scenario | sysctl/environment evidence | minimal syscall evidence | Bubblewrap evidence | Classification |
|---|---|---|---|---|
| user namespaces globally disabled | `user.max_user_namespaces=0`, `unprivileged_userns_clone=0` | `unshare -Ur` fails with `EPERM` | kernel does not allow non-privileged user namespaces | `userns_disabled` |
| Ubuntu 24.04 AppArmor | max namespaces nonzero, userns clone enabled, `apparmor_restrict_unprivileged_userns=1` | writing `/proc/self/uid_map` fails with `EPERM` | `setting up uid map: Permission denied` | `apparmor_uid_map_denied` |
| Podman restricted container | `/.containerenv` or `/run/.containerenv`, no restrictive host userns sysctl | `unshare -Ur` succeeds | mounting `/proc` fails with `EPERM` | `mount_proc_denied` in `container` context |

The Podman case does not prove that seccomp blocked `clone` or `unshare`; in the
observed environment unprivileged user namespace creation succeeded and the
failure moved to mounting `/proc`. Reason codes therefore describe the first
observable failed stage:

- `bwrap_missing`
- `userns_disabled`
- `uid_map_denied`
- `mount_denied`
- `unknown`

Container markers are context, not proof of the failing mechanism. An
unclassified result must retain stderr plus a sysctl/environment snapshot.
The Ubuntu 22.04 sysctl values were restored after the disabled-userns probe.

The classifier and offline fixtures are implemented in the worker sandbox
module. The real matrices now assert:

- missing `bwrap` -> `bwrap_missing`;
- Podman -> `mount_denied` with target `/proc`;
- Ubuntu 24.04 AppArmor -> `uid_map_denied`;
- unknown stderr -> `unknown` while retaining a bounded, sanitized snapshot.

Protocol errors expose stable `details.reason` with diagnostic context.
Asynchronous `AGENT_EXEC` startup failures expose the same shape through
`error_details`. A capability can succeed, then lose Bubblewrap before exec;
the command must still fail with `bwrap_missing`.

### Mount layout

The adapter mounts existing common system roots read-only, provides private
`/proc`, `/dev`, and `/tmp`, then applies the workspace bind and protected-path
overlays. A protected path must already exist and remain inside the canonical
workspace. This prevents a profile from silently claiming protection for a path
that Bubblewrap cannot mount.

## Verification

- `just stage10-sandbox`: pass.
- `just stage9-agent-exec`: pass, covering the pre-sandbox agent regression.
- `just bubblewrap-poc`: pass, 5 tests.
- `just sandbox-container-matrix`: Ubuntu 22.04, Ubuntu 24.04, and Debian 12
  have installed Bubblewrap, reject the nested mount/path setup imposed by
  Podman, and return `sandbox_backend_unavailable` without fallback.
- `just sandbox-vm-matrix`:
  - Ubuntu 22.04 and Debian 12 pass the complete real-VM Stage 10 probe;
  - Ubuntu 24.04 has Bubblewrap installed but rejects user-ID mapping under the
    tested AppArmor/userns policy;
  - qwed returns `sandbox_backend_unavailable` on Ubuntu 24.04 and does not
    execute the command.
- `just sandbox-userns-disabled-vm`: temporarily sets the Ubuntu 22.04
  namespace sysctls to zero, verifies reason `userns_disabled`, and restores
  the original sysctl values through a shell trap.
- `just sandbox-systemd-properties`: runs Bubblewrap 0.13.1 under the packaged
  `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`, and
  `PrivateTmp` properties. RW workspace writes pass under both `/run/user` and
  `$HOME`.

The container matrix verifies fail-closed deployment behavior only. It does not
claim that Bubblewrap succeeds inside a restricted container.

## Not Yet Validated

- a successful Ubuntu 24.04 configuration with an approved AppArmor/userns
  policy;
- other distributions and kernels with user namespaces explicitly disabled; and
- seccomp, Landlock, and stronger VM/OCI isolation.
