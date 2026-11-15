# Experiment 0051: Sandboxed Pane PoC Gates

Date: 2026-11-15
Status: P1-P6 pass

Run P2-P4 contract probes with:

```sh
just sandbox-pane-poc
```

## P1: Bubblewrap FD Binding

### Method

Use tagged upstream Bubblewrap sources rather than version-string inference:

```text
v0.9.0  bubblewrap.c sha256 6a89bc9b7a794d19e2bdb1e139ab9d5bba84e807b330336f363f88f130cfeab6
v0.10.0 bubblewrap.c sha256 f2d54347aa4949ec3a399fd362a6f271df70a6622c8338c9404601e694d518a9
```

Search for the option parser and help entries for `--bind-fd` and
`--ro-bind-fd`.

### Result

| Version | `--bind-fd`/`--ro-bind-fd` |
|---|---|
| 0.5.0 | absent |
| 0.6.x | absent |
| 0.7.0 | absent |
| 0.8.0 | absent |
| 0.9.0 | absent |
| 0.10.0 | present |
| 0.11.x | present |
| 0.12.x | present |
| 0.13.1 | present |

The first observed upstream release is **0.10.0**. The version is supporting
evidence, not the runtime contract. Sandboxed Pane must probe the exact help
contract and a harmless FD bind, because distributions may backport or remove
features independently of the version string.

The 0.10.0 implementation resolves the FD through a magic symlink, mounts it,
then compares `fstat(fd)` with `stat(mount_destination)`. A mismatch aborts with
the equivalent of `Race condition binding dirfd`. QWE should retain that
Bubblewrap check and must not replace it with path-only assumptions.

### Decision Impact

- `AGENT_EXEC` keeps Bubblewrap 0.5 compatibility.
- Sandboxed Pane requires FD-bind help entries plus a successful harmless FD
  bind probe.
- The probe result is part of backend feature capability, not profile trust.
- A backend missing FD binding returns `sandbox_backend_unavailable` with the
  missing feature named.

The worker now performs the harmless FD mount during `SANDBOX_PROBE`, reports
`fd_bind`, and qwed persists it in capability sandbox audit metadata. The
current `AGENT_EXEC` path does not require the feature. Sandboxed-Pane
capability issue now uses an ephemeral worker to probe the Session-scoped
profile before a Pane exists. Backends without FD binding remain usable for
`AGENT_EXEC` but fail `sandboxed_pane` issue with
`sandbox_backend_unavailable` / `fd_bind_unavailable`.

## P2: Nested Sandbox

Status: pass.

Questions:

- Does QWE's strict syscall denylist permit or deny nested Bubblewrap?
- Which kernel/namespace operations fail first?
- If Bubblewrap cannot nest, are shim-stacked Landlock, seccomp, and
  `no_new_privs` sufficient for only-tighten behavior?
- Can network socket creation be denied without changing existing sockets?

### Result

- strict seccomp alone does **not** prevent nested Bubblewrap;
- a nested Bubblewrap succeeds when the outer QWE profile uses
  `landlock:off` and the strict syscall denylist;
- with QWE's normal Landlock write rules, nested Bubblewrap fails while writing
  `/proc/self/uid_map`; the inner setup cannot preserve the outer policy;
- therefore nested Bubblewrap is not a reliable monotonic mechanism after
  Landlock is applied.

The supported design direction is shim-stacked Landlock, seccomp, and
no-new-privileges. D15 is resolved.

A dedicated seccomp filter denied `socket(2)` with `EPERM` while an inherited
Unix stream socket continued to send data. D16 is therefore explicit: socket
creation can be denied monotonically, but existing FDs remain usable.

The same nested-success/Landlock-denial pair also passes Stage 10 with the
Bubblewrap 0.5.0 matrix on Ubuntu 22.04, Ubuntu 24.04 with AppArmor, and Debian
12.

## P3: SIGWINCH and Foreground Process Group

Status: pass.

Questions:

- Does `TIOCSWINSZ` on the qwed-owned PTY master deliver SIGWINCH to the
  foreground process group inside the PID namespace?
- Does `tcgetpgrp` report the expected process group from the shim?
- Can winsize synchronization remain outside the shim?

### Result

The Bubblewrap contract test traps SIGWINCH inside an interactive shell after
`TIOCSWINSZ` on the host-owned PTY master. The shell receives `WINCH_SEEN`, and
`tcgetpgrp(master)` remains stable across resize. Kernel terminal handling is
sufficient; the shim does not need to forward window size.

## P4: Shim PID-1 Behavior

Status: pass.

Questions:

- Are SIGTERM and SIGINT delivered to a PID-1 shim without explicit handlers?
- Does SIGCHLD handling reap all descendants without creating zombies?
- What happens when foreground process and shell both exit?

### Result

- a PID-1 process with no SIGTERM handler ignores SIGTERM;
- explicit SIGTERM and SIGINT handlers receive both signals;
- an explicit SIGCHLD handler reaps the child without leaving a zombie;
- SIGINT observation may terminate the outer wrapper by SIGINT even after the
  target's handler runs, so shutdown status must not be inferred from wrapper
  exit alone.

The first sandboxed-Pane shim must explicitly install SIGTERM, SIGINT, and
SIGCHLD handling.

The initial Rust shim now runs under a real PTY inside a PID namespace. It
reuses the inherited controlling terminal, creates the shell process group,
assigns it as the PTY foreground group, forwards TERM/INT/HUP, reaps children,
and terminates a background namespace process after shell exit.

The same real-PTY contract connects to a versioned, bounded Pane control
channel through `qwe-session-exec`. The shim forwards permitted control signals,
rejects malformed frames, and marks the control FD close-on-exec so the shell
cannot access it.

## Production Slice Evidence

`just stage10-sandbox` now creates and recovers a long-lived sandboxed Pane
through qwed. The observed path is:

- Session-scoped `sandboxed_pane` capability issue with profile/audit;
- `SESSION_CREATE` and `PANE_CREATE` capability consumption without client
  profile or long-lived credential input;
- launcher-pinned workspace/protected/HOME descriptors, Bubblewrap PID
  namespace, internal seccomp, `--pane-init`, and private control channel;
- atomic Pane sandbox metadata persistence and post-restart status recovery;
- capability-downgrade rejection, target-graded `pane_input`, and temporary
  filtered read-only observation;
- restricted output filtering before journal/VT/Attach retention, with OSC
  title/clipboard and terminal-query sequences removed; and
- bounded metadata-only audit before sandboxed `SESSION_INPUT`, with content
  absent from SQLite; and
- interactive Attach capability binding plus per-Input audit, while unmarked
  direct worker Attach input remains fail closed; and
- standalone `SESSION_RESIZE` capability authorization with observed PTY
  dimension changes; and
- shared process namespace Pane creation with hard cgroup scope, cgroup-only
  control, host child-PID monitoring, and detached-process teardown on shell
  exit; and
- `pane_exec` enforcement with inherited parent profiles and construction-time
  only-tighten refinement; and
- durable orphan TTL enforcement, orphan-count admission control, and
  termination-level Principal revocation of affected sandboxed Panes.
- restricted output filtering for Agent command stdout, stderr, and PTY
  streams before cursor advancement and event retention, after credential
  redaction; and
- a production Pane control decoder shared by the shim and a `cargo-fuzz`
  target, with deterministic exhaustive input tests for both the control
  decoder and restricted terminal filter.

The shared fuzz harnesses are run with:

```sh
just fuzz-sandbox-control 10000
just fuzz-terminal-output 10000
```

On 2026-11-16, both targets completed 10,000 nightly runs without a crash or
counterexample. The deterministic exhaustive tests remain in the normal
workspace test suite so parser/filter regressions do not depend on a local
fuzz-toolchain installation.

Keep fuzz harnesses under `fuzz/fuzz_targets/` and import production parsers
and state machines from `qwe-runtime`; do not duplicate their state in a
harness.

## Current x86_64 Distribution Requalification

On 2026-11-16, the current sandboxed-Pane and `pane_exec` implementation was
run against the configured real VMs:

| Target | Result |
|---|---|
| Ubuntu 22.04 | complete Stage 10 pass |
| Debian 12 | complete Stage 10 pass |
| Ubuntu 24.04 default | fail closed with `sandbox_backend_unavailable`; ordinary Pane pass |
| Ubuntu 24.04 packaged AppArmor | complete Stage 10 pass |
| Fedora 44, SELinux `Enforcing` | complete Stage 10 pass |

The same current revision also reran the pinned Bubblewrap 0.5.0 matrix.
Ubuntu 22.04, Debian 12, and Ubuntu 24.04 with the packaged AppArmor profile
pass every applicable `AGENT_EXEC` check. Bubblewrap 0.5.0 has no FD bind, so
the sandboxed-Pane paths correctly report `sandbox_backend_unavailable` /
`fd_bind_unavailable` and remain `unavailable` rather than falling back to an
unsandboxed Pane. This requalifies
`SANDBOX-017` against the current probe.

The current `pane_exec` fixture injects OSC title and OSC 52 payloads and
requires the Agent stdout event to equal only `PANE_EXEC_INHERITED`. This
covers `SANDBOX-052` on the local and supported remote paths.

## P5: Landlock ABI Distribution

Status: pass.

`just sandbox-landlock-matrix` runs the ABI/rights probe on the Ubuntu and
Debian VMs. Fedora 44 is qualified separately.

| Target | Kernel | Landlock ABI | `REFER` | `TRUNCATE` | `IOCTL_DEV` |
|---|---|---:|---|---|---|
| Fedora 35 host | 5.14 | 1 | ABI-1 conflict enforcement | no | no |
| Ubuntu 22.04 | 5.15 | 1 | ABI-1 conflict enforcement | no | no |
| Debian 12 | 6.1 | 2 | yes | no | no |
| Ubuntu 24.04 | 6.8 | 4 | yes | yes | no |
| Fedora 44 | 6.19 | 7 | yes | yes | yes |

The sandboxed-Pane `required{min_abi}` default should use the filesystem ABI
needed by its rules, not the highest observed ABI. ABI-sensitive tests remain
mandatory when the minimum changes.

## P6: Seccomp Runtime Compatibility

Status: pass for the current strict denylist.

Stage 10 now checks strict mode with:

- Python 3.10, 3.12, and 3.14;
- threads, subprocess, file stat/access, pipes, and SSL initialization;
- the Rust `qwe-sandbox-init --version` startup path;
- shell and nested Bubblewrap behavior;
- glibc-based Ubuntu and Debian;
- Fedora SELinux enforcing.

The current profile is a small denylist, so it does not inherit the
`clone3`/`rseq`/`statx` compatibility risk of a syscall allowlist. Any future
allowlist or broader named profile still requires its own target-runtime matrix.

## Follow-up

P1-P6 update ADR 0034. Nested Bubblewrap is not the refinement mechanism after
Landlock; shim stacking is adopted. Socket denial affects new sockets only.
The next work is code implementation: FD-bind probe, typed profile refinement,
capability operations, and the sandboxed-Pane shim.
