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
- `workspace:read_write` can persist an inherited FD3 credential in a
  host-visible workspace file after the command exits;
- an existing protected path inside a writable workspace remains read-only;
- protected-path symlinks resolve inside the canonical workspace and remain
  read-only;
- host `$HOME` is absent and `/tmp` is private;
- host PIDs, `/etc/shadow`, `/sys`, and host environment values are absent;
- workspace symlinks to host paths outside the mount policy are inaccessible;
- isolated networking has no default route, keeps `lo` UP, and supports a
  local `127.0.0.1` listen/connect pair;
- `NoNewPrivs: 1` makes setuid privilege gain denial an observable guarantee;
- on Landlock-capable kernels, the optional init applies write confinement
  inside Bubblewrap and records the detected ABI in capability audit metadata;
- on Ubuntu 24.04 Landlock ABI 4, `REFER` blocks cross-workspace rename and
  `TRUNCATE` blocks truncation outside allowed write roots;
- FD3 credentials and exact-match redaction remain intact;
- wrapper and structured child PIDs both inherit the Pane workload cgroup;
- sandboxed PTY commands preserve terminal size, FD3 credentials, redaction,
  foreground idle, termination, and cgroup ownership;
- a seccomp filter rejects `TIOCSTI` with `EPERM` in pipe-split and PTY modes
  while preserving normal PTY operation;
- the `strict` named seccomp profile rejects its configured high-risk syscalls
  with stable return codes in pipe-split and PTY commands, and its
  content-addressed filter hash is present in capability audit metadata;
- an unavailable Bubblewrap executable fails capability issue closed;
- a backend with an acceptable `--version` but an incomplete production
  `--help` option set fails capability issue with
  `bwrap_version_incompatible`;
- when Landlock is available but `qwe-sandbox-init` is missing from an isolated
  binary directory, capability issue fails with `sandbox_init_unavailable`;
- Ubuntu 24.04 keeps `apparmor_restrict_unprivileged_userns=1` while the
  packaged path-scoped profile grants `userns` only to supported Bubblewrap
  paths, and the complete Stage 10 probe passes;
- Fedora 44 passes the complete probe with SELinux confirmed `Enforcing`;
- Bubblewrap 0.5.0 built from the pinned source archive passes the complete
  probe on Ubuntu 22.04, Ubuntu 24.04 under AppArmor, and Debian 12;
- qwed restart reconnects to the same worker, wrapper, child, profile, and
  events;
- forced Pane teardown removes the worker, wrapper, child, descendants, and
  cgroup; and
- worker loss marks commands `lost`, preserves the enforced profile, and
  removes the sandbox process tree.

The confirmed `network:inherit + FD3 credential` transfer and
`workspace:read_write + FD3 credential` persistence are known-risk capability
boundaries, not redaction bugs. Command status exposes structured `sandbox.risk`
flags and events include `sandbox_risk_observed`. Current risk events and the
base risk flags are persisted in SQLite, so they survive worker loss and qwed
restart. Event replay itself remains worker-memory bounded.

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
| Ubuntu 24.04 | 0.9.0 | unprivileged, packaged path-scoped AppArmor profile | `0x49` | pass | pass |

`0x49` contains both `IFF_UP` and `IFF_LOOPBACK`. Ubuntu 24.04 keeps
`kernel.apparmor_restrict_unprivileged_userns=1`; the approved profile grants
`userns` only to Bubblewrap.
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

### Ubuntu 24.04 AppArmor profile

The Ubuntu 24.04 default policy still produces `uid_map_denied`, which is the
expected fail-closed result when no profile grants user-namespace creation.
The approved deployment asset is a path-scoped AppArmor profile for Bubblewrap,
not a global sysctl relaxation.

The packaged profile attaches to `/usr/bin/bwrap`, `/usr/local/bin/bwrap`, and
`/opt/local/bin/bwrap` with `flags=(unconfined)` plus the `userns` rule.
`just sandbox-apparmor-vm` installs that profile, asserts all three attachment
profiles are loaded and the global restriction remains `1`, then runs the full
Stage 10 probe. The probe restores the VM's previous profile state afterward.
A custom `QWE_BWRAP_PATH` outside the documented paths requires an
administrator-reviewed extension.

### SELinux and oldest Bubblewrap

Fedora 44 provides a real SELinux enforcing qualification. `SANDBOX-016`
asserts `/sys/fs/selinux/enforce=1` and `getenforce=Enforcing` before and after
the complete Stage 10 probe; the distribution Bubblewrap 0.12.0 runs without
disabling SELinux or adding a permissive domain.

The tested VM used Fedora Cloud Base Generic 44-1.7 x86_64 with SHA-256
`28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f`,
kernel `6.19.10-300.fc44.x86_64`, and the `bubblewrap`, `python3`, and
`policycoreutils` packages.

The minimum-version matrix pins Bubblewrap 0.5.0 source by SHA-256
`9910917c031bc4305e5620e53a602161fdd676de9c9f7da5c082a508a880912c`. The recipe
builds and installs that version on Ubuntu 22.04, Ubuntu 24.04, and Debian 12,
then runs the complete Stage 10 probe. Ubuntu 24.04 also loads the packaged
AppArmor profile, so the oldest Bubblewrap version is qualified without
turning off the global userns restriction.

Each VM temporarily replaces `/usr/local/bin/bwrap`, verifies the exact
`bubblewrap 0.5.0` version, and restores the previous binary/profile in a shell
trap.

Capability issue separately checks that `bwrap --help` lists every option used
by the production adapter. This catches backends that can start simple
namespaces but cannot support `--json-status-fd`, `--seccomp`, the required
bind policy, environment controls, or lifecycle flags. A missing option is
classified as `bwrap_version_incompatible` before a capability token is
returned. This is a runtime contract probe, not a claim that every supported
release has already passed the complete behavior matrix.

Protocol errors expose stable `details.reason` with diagnostic context.
Asynchronous `AGENT_EXEC` startup failures expose the same shape through
`error_details`. A capability can succeed, then lose Bubblewrap before exec;
the command must still fail with `bwrap_missing`.

Diagnostics visibility is capability-principal based. High-trust callers see
the full bounded snapshot. Low-trust callers retain stable reason/environment/
failed-stage/target fields, but not stderr, backend path/version, probe time, or
sysctl values.

### Named seccomp profile

Profile v1 now accepts the exact named seccomp profile `strict`. The profile is
a small denylist chosen to preserve common runtime fallback behavior:

- `ptrace`, `bpf`, `keyctl`, `add_key`, `request_key`, and `userfaultfd`
  return `EPERM`; and
- `io_uring_setup` returns `ENOSYS`.

The worker rejects unknown names before issuing a token. The capability audit
stores `seccomp_profile_hash`, the SHA-256 of the exact cBPF byte sequence
passed to Bubblewrap. This means the stable profile name does not hide future
filter changes: a capability records the bytes that were actually authorized.

The Stage 10 probe exercises both pipe-split and PTY execution under `strict`.
It asserts the specified errno results, confirms the strict hash differs from
the base adapter filter, and verifies the hash survives in live sandbox audit
metadata. `strict` remains a denylist rather than a complete syscall sandbox.

### Landlock defense in depth

The worker probes Landlock ABI support independently of Bubblewrap. When ABI 1
or newer is available, Bubblewrap binds `qwe-sandbox-init` into the sandbox and
executes it as the command wrapper. The init sets `PR_SET_NO_NEW_PRIVS`, applies
an ABI-aware write-focused Landlock ruleset, records `QWE_LANDLOCK_ABI`, and
immediately execs the requested program.

The ruleset handles write, remove, and create operations while leaving reads,
directory listing, and execution unhandled. Writes are allowed under private
`/tmp`, `/dev`, and a read-write workspace. A read-only workspace receives no
write rule. Bubblewrap remains mandatory and authoritative for mount
visibility and protected paths; this is not a Landlock-only fallback.

The handled access mask grows with the kernel ABI. ABI 2 handles `REFER`, ABI 3
handles `TRUNCATE`, and ABI 5 handles device `IOCTL`. ABI 4 network rights are
not handled because network confinement remains Bubblewrap's responsibility.

`just landlock-poc` verifies RW and RO workspace behavior, denial outside
allowed write roots, and preservation of reads and private `/tmp`. Stage 10
confirms the same helper is applied through the production Bubblewrap and
PTY process chains, and that capability audit metadata records the ABI. The
Ubuntu 22.04 VM kernel also exposes Landlock.

`just sandbox-landlock-vm ubuntu2404` runs the init directly on the ABI 4
Ubuntu kernel. Cross-workspace rename is denied through `REFER`, truncation is
denied through `TRUNCATE`, workspace and private `/tmp` writes still pass, and
the result records `ioctl_dev:not_supported` for ABI 4.

Capability issue validates that the init helper exists, is executable, and is
adjacent to the worker whenever Landlock is available. The Stage 10 probe runs
qwed from a copied binary directory that omits the helper and requires
`sandbox_backend_unavailable` with reason `sandbox_init_unavailable` before a
token is returned.

### Mount layout

The adapter mounts existing common system roots read-only, provides private
`/proc`, `/dev`, and `/tmp`, then applies the workspace bind and protected-path
overlays. A protected path must already exist and remain inside the canonical
workspace. This prevents a profile from silently claiming protection for a path
that Bubblewrap cannot mount.

## Verification

- `just stage10-sandbox`: pass, including the named `strict` seccomp profile,
  incompatible-backend feature probe, and missing-init fail-closed probe.
- `just stage9-agent-exec`: pass, covering the pre-sandbox agent regression.
- `just bubblewrap-poc`: pass, 5 tests.
- `just landlock-poc`: pass, 2 contract tests plus the host ABI probe.
- `just sandbox-landlock-vm ubuntu2404`: pass, ABI 4 with `REFER` and
  `TRUNCATE` enforced.
- `just sandbox-container-matrix`: Ubuntu 22.04, Ubuntu 24.04, and Debian 12
  have installed Bubblewrap, reject the nested mount/path setup imposed by
  Podman, and return `sandbox_backend_unavailable` without fallback.
- `just sandbox-vm-matrix`:
  - Ubuntu 22.04 and Debian 12 pass the complete real-VM Stage 10 probe;
  - default Ubuntu 24.04 rejects user-ID mapping under the system-wide
    AppArmor/userns policy and qwed fails closed.
- `just sandbox-apparmor-vm`: installs the packaged path-scoped profile on
  Ubuntu 24.04, verifies all three supported Bubblewrap attachment paths load
  while the global restriction remains enabled, and passes the complete Stage
  10 probe.
- `just sandbox-userns-disabled-vm-matrix`: temporarily sets the namespace
  sysctls to zero on Ubuntu 22.04, Ubuntu 24.04, and Debian 12, verifies reason
  `userns_disabled`, executes an ordinary Pane command successfully, and
  restores the original sysctl values through a shell trap.
- `just sandbox-selinux-vm`: Fedora 44 remains SELinux `Enforcing` throughout
  the complete Stage 10 probe.
- `just sandbox-bubblewrap-oldest-vm-matrix`: Bubblewrap 0.5.0 passes the full
  probe on Ubuntu 22.04, Ubuntu 24.04 with AppArmor, and Debian 12.
- `just sandbox-systemd-properties`: runs Bubblewrap 0.13.1 under the packaged
  `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`, and
  `PrivateTmp` properties. RW workspace writes pass under both `/run/user` and
  `$HOME`.
- `just sandbox-systemd-service-smoke`: starts qwed in a transient user service
  with packaged security properties, runs sandboxed Agent commands, kills and
  restarts qwed, atomically replaces the qwed binary, restarts again, executes
  another sandbox command, stops the service, and verifies this smoke's Pane
  scopes are no longer active.

The container matrix verifies fail-closed deployment behavior only. It does not
claim that Bubblewrap succeeds inside a restricted container.

## Not Yet Validated

- additional SELinux-enforcing distributions and kernels with user namespaces
  explicitly disabled;
- additional named seccomp profiles, broader runtime compatibility coverage,
  Landlock ABI 5+ device-ioctl behavior, and stronger VM/OCI isolation.
