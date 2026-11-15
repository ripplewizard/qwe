# ADR 0034: Sandboxed Pane, Interactive PTY, and Path Trust

Status: Accepted for the next sandbox milestone; implementation not started
Date: 2026-11-15
Extends: [ADR 0032](0032-agent-execution-model.md),
[ADR 0033](0033-bubblewrap-sandbox-adapter.md)

## Context

The Linux `AGENT_EXEC` Bubblewrap slice is implemented and qualified. Two
remaining decisions share the same terminal machinery but have different
authority:

- whether a long-lived Pane shell is sandboxed; and
- whether an Agent command may interact with a PTY through input and resize.

The current `qwe-sandbox-init` applies Landlock and immediately execs. It does
not establish a controlling terminal, forward signals, or synchronize window
size. Extending the current `AGENT_EXEC` pipe path into a long-lived Pane would
therefore bypass unresolved terminal semantics.

This ADR also fixes the host-path trust boundary. A same-UID host process is
trusted and can ptrace or replace qwed state. A sandboxed Agent is not trusted
to choose the inode behind paths that the host subsequently uses.

## Decision

### 1. Same-UID host processes remain outside the threat model

The single-user development model treats qwed and other processes under the
same UID as mutually trusting. QWE does not claim protection against a
same-UID process that ptraces qwed, rewrites its configuration, or replaces its
runtime paths.

Sandbox-originated workspace content remains untrusted. Protected paths,
workspace cwd, and hardlink inspection therefore use
`openat2(RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS)`:

- paths are lexically required to remain below the workspace;
- absolute symlinks are rejected by `RESOLVE_BENEATH`;
- relative symlinks may resolve only within the workspace;
- protected paths containing multi-link files fail closed; and
- the worker repeats validation rather than trusting qwed's earlier result.

The Bubblewrap CLI still accepts resolved path strings. Bubblewrap 0.5 does not
support `--ro-bind-fd`, so a same-UID replacement window between final
canonicalization and Bubblewrap consuming the source path remains an explicit
residual risk. Full FD-pinned mount sources require a later minimum-version
decision and `--ro-bind-fd`.

### 2. Sandboxed Pane is a separate capability and lifecycle

Default Panes remain unsandboxed for platform compatibility. A Pane shell can
be sandboxed only through an explicit management-issued capability:

- `sandboxed_pane` authorizes creating the Pane in a sandbox;
- `pane_exec` authorizes injecting subsequent commands into an existing Pane;
- `SESSION_CREATE.sandbox_profile` remains invalid as ambient selection; and
- an Agent cannot select or broaden its profile through `AGENT_EXEC`.

The implementation uses a long-lived Bubblewrap invocation and an A+ shim:

```text
qwe-worker
  -> qwe-session-exec
       -> bwrap
            -> qwe-sandbox-init
                 -> shell
```

For a Pane, `qwe-sandbox-init` additionally owns controlling-terminal setup,
signal forwarding, foreground process-group tracking, and window-size
synchronization. `exec`-replacing the shell does not change the sandbox or
shim policy.

A sandboxed Pane and its descendants remain in the Pane cgroup. qwed restart,
Attach reconnect, worker loss, and Pane teardown must preserve the same
ownership semantics as an ordinary Pane.

### 3. Nested sandbox policy can only tighten

An `AGENT_EXEC` launched inside a sandboxed Pane must satisfy both profiles.
The effective profile is the intersection of the Pane and command policies.
Profile ordering must be represented in types and construction, not only as a
runtime string comparison. A child profile that broadens workspace, network,
HOME, PID, protected paths, or seccomp capability is rejected before spawn.

### 4. Interactive Agent PTY input/resize ships with the Pane milestone

PTY support without input or resize can leave Agents blocked on prompts,
pagers, editors, and confirmation screens. If sandboxed Pane is accepted,
interactive Agent PTY control is designed in the same milestone.

- Input and resize use distinct capability-protected operations.
- Input is bounded and returns an explicit overload error instead of blocking
  qwed's event loop.
- Audit records length and time by default; raw input retention requires an
  explicit opt-in.
- Raw input remains authorized per command or Pane and never becomes ambient
  Process observation authority.

Untrusted PTY output is not automatically trusted terminal control. OSC 52
clipboard writes and other host-sensitive control sequences are filtered or
disabled by default at the human projection boundary. Query-response
sequences must not create an unbounded input echo path.

### 5. A sandboxed Pane does not retain command credentials

FD3 remains scoped to one `AGENT_EXEC`. A long-lived Pane shell does not inherit
FD3 and cannot keep a credential readable for the Pane lifetime. Commands that
need credentials receive a fresh descriptor through the existing per-command
broker.

## Consequences

- Pane sandboxing does not reuse the pipe-split implementation as an unqualified
  replacement.
- `qwe-sandbox-init` becomes trusted computing base code for interactive
  sessions and requires focused review, fuzzing, and minimal dependencies.
- The initial path implementation rejects absolute symlinks in protected paths
  and cwd.
- Same-UID host races remain documented residual risk; sandbox-originated path
  aliases and absolute-symlink escapes fail closed.
- Interactive terminals require output-control filtering in addition to input
  authorization.

## Validation

Before implementation acceptance:

- deterministic terminal tests for ctty, SIGINT, SIGTSTP, SIGWINCH, resize,
  job control, and foreground process groups;
- nested profile refinement tests proving only-tighten behavior;
- qwed restart, attach, worker loss, and Pane teardown;
- executable replacement inside the shell without changing sandbox policy;
- FD3 absence from a long-lived Pane and per-command credential delivery;
- bounded input, overload, observer denial, and audit behavior;
- OSC 52 and terminal-query response filtering; and
- fuzz coverage for `qwe-sandbox-init` argument parsing, Landlock setup, and
  exec handoff.

## References

- [ADR 0032](0032-agent-execution-model.md)
- [ADR 0033](0033-bubblewrap-sandbox-adapter.md)
- [Feature Assessment](../feature-assessment-agent-runtime-and-ux.md)
- [`qwe-sandbox-init` Threat Model](../sandbox-init-threat-model.md)
- [Linux openat2](https://man7.org/linux/man-pages/man2/openat2.2.html)
