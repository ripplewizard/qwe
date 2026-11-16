# Experiment 0048: Bubblewrap Contract PoC

Date: 2026-11-15
Status: Pass for core adapter contract

## Environment

- Linux x86_64
- Bubblewrap 0.5.0
- systemd 249
- unprivileged process with no effective capabilities

The Rust contract probe is
`apps/qwe-worker/tests/bubblewrap_contract.rs`. It skips when `bwrap` is absent,
so `cargo test --workspace` remains portable.

## Results

### FD3, cgroup, and child identity

Bubblewrap:

- preserved a deliberately inherited FD3 byte stream;
- inherited the caller's cgroup;
- exposed `cgroup.kill` in the workload cgroup for final teardown;
- accepted `--json-status-fd`;
- reported a host-visible `child-pid` distinct from the outer Bubblewrap PID;
- successfully produced the actual command process in the same cgroup; and
- returned the command's exit status.

### PTY and foreground group

An interactive bash shell ran in a real Rust `portable-pty`.

With `--new-session`:

- startup succeeded;
- the foreground group mapped to outer Bubblewrap; but
- bash reported “no job control in this shell”.

Without `--new-session`:

- the initial `stty size` reported `24 80`;
- resizing the PTY to `40 x 120` was visible immediately;
- Ctrl-C interrupted the foreground `sleep` without killing the shell;
- Ctrl-Z stopped the foreground job; and
- `jobs` reported the stopped job.

Direct bwrap takes precedence over the proposed shim only for non-PTY commands.
Interactive PTY support remains gated on `TIOCSTI` seccomp and explicit mapping
of Bubblewrap's actual child and PTY foreground groups.

### Parent death

An immediate spawning-thread exit raced with Bubblewrap startup and was not a
valid test. After waiting for the structured startup record, exiting the
creator thread caused the sandbox and sleeping command to terminate within the
two-second probe deadline.

The implementation requirement is therefore:

1. spawn from a dedicated command lifetime thread;
2. wait for Bubblewrap startup JSON;
3. keep the creator thread alive until terminal state.

## Verification

```sh
just bubblewrap-poc
```

Current result: 5 passed, 0 failed.

## Not Yet Validated

- Ubuntu 24.04 AppArmor-denied unprivileged user namespaces;
- Bubblewrap minimum-version and flag matrix;
- seccomp BPF through an independent FD;
- filesystem and network escape probes;
- writable workspace protected-path enforcement;
- credential exfiltration through network or files;
- qwed/worker restart recovery and Pane cleanup; and
- real QWE `AGENT_EXEC` profile authorization and integration.
