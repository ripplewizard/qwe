# Experiment 0051: Sandboxed Pane PoC Gates

Date: 2026-11-15
Status: P1 pass; P2-P4 pending

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

## P2: Nested Sandbox

Status: pending.

Questions:

- Does QWE's strict syscall denylist permit or deny nested Bubblewrap?
- Which kernel/namespace operations fail first?
- If Bubblewrap cannot nest, are shim-stacked Landlock, seccomp, and
  `no_new_privs` sufficient for only-tighten behavior?
- Can network socket creation be denied without changing existing sockets?

## P3: SIGWINCH and Foreground Process Group

Status: pending.

Questions:

- Does `TIOCSWINSZ` on the qwed-owned PTY master deliver SIGWINCH to the
  foreground process group inside the PID namespace?
- Does `tcgetpgrp` report the expected process group from the shim?
- Can winsize synchronization remain outside the shim?

## P4: Shim PID-1 Behavior

Status: pending.

Questions:

- Are SIGTERM and SIGINT delivered to a PID-1 shim without explicit handlers?
- Does SIGCHLD handling reap all descendants without creating zombies?
- What happens when foreground process and shell both exit?

## Follow-up

P1 result updates ADR 0034 D1. P2-P4 remain implementation gates for sandboxed
Pane; no Pane sandbox code should assume their outcomes.
