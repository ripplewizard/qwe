# `qwe-sandbox-init` Threat Model and Fuzz Plan

Date: 2026-11-16
Status: Current for `AGENT_EXEC` and the production sandboxed-Pane shim

## 1. Role in the trust boundary

`qwe-sandbox-init` executes inside mandatory Bubblewrap confinement, immediately
before the requested command. It is trusted computing base code because it:

- receives worker-generated profile arguments;
- applies `PR_SET_NO_NEW_PRIVS` and Landlock;
- disables its own process dumpability;
- exports the successful Landlock ABI; and
- replaces itself with the requested program.
- in the hidden `--pane-init` mode, remains as PID namespace init, owns the
  inherited controlling terminal, assigns the foreground process group,
  forwards TERM/INT/HUP, consumes a versioned signal control channel, and
  reaps descendants before exiting.

It does not load the Bubblewrap seccomp program. Bubblewrap applies seccomp
before executing the init helper. The terminal/signal and private-control-
channel behavior from ADR 0034 has a dedicated real-PTY contract test.
Production creation now runs this path for isolated-process-namespace
sandboxed Panes and uses the worker-owned cgroup for shared-process-namespace
teardown. Restricted terminal Attach/input is implemented, and the Pane control
decoder lives in `qwe-runtime` so the shim and fuzz target exercise the same
production parser.

## 2. Trusted inputs

- `--workspace` and `--private-tmp` come only from the validated capability
  profile injected by qwed and checked again by the worker.
- `--workspace-mode` comes from the same profile.
- The executable and trailing arguments come from `AGENT_EXEC`; they are
  untrusted program data but cannot change init's own options after `--`.
- Landlock ABI and syscall support come from the kernel.

No credential, environment secret, or host configuration file is an init input.

## 3. Required invariants

1. No target program executes if argument conversion, Landlock setup,
   no-new-privileges, dumpability hardening, or exec preparation fails.
2. Initialization failure exits with code 125 and a diagnostic; it never falls
   back to unsandboxed execution.
3. Landlock is applied before `execvp`.
4. `QWE_LANDLOCK_ABI` is set only after successful Landlock application.
5. The helper retains no files or credentials beyond standard descriptors.
6. Absolute symlinks cannot escape the workspace under path resolution.
7. Landlock failure is hard failure; unsupported ABI is handled before init by
   the worker choosing whether the optional layer is available.
8. A profile with Landlock mode `required{min_abi}` never starts when the
   observed ABI is lower; there is no best-effort fallback inside that mode.
9. Pane-init signal handlers only record pending signals and wake the main
   loop; forwarding and process reaping remain in normal control flow.
10. Shell exit starts bounded descendant teardown with SIGTERM and escalates to
    SIGKILL after the frozen grace period before the shim exits.

## 4. Threats and current controls

| Threat | Current control |
|---|---|
| Malformed/missing options | Clap rejects before sandbox initialization |
| NUL in executable/arguments | `CString` conversion fails closed |
| Workspace path escape | Worker validation and `openat2(RESOLVE_BENEATH)` for profile paths |
| Landlock setup failure | Process exits 125 before target exec |
| Same-sandbox ptrace of init | `PR_SET_DUMPABLE=0` after Landlock setup |
| Core dump of helper state | Dumpability disabled before target exec |
| Seccomp absent because init failed | Target is never exec'd |
| Silent ABI downgrade | Ruleset mask derives from the observed ABI; unsupported future bits remain unhandled and are documented |
| Shell replacement changing policy | Future Pane shim remains the sandbox parent and does not re-enter policy selection |
| Landlock meets a required ABI | `required{min_abi}` is checked before init; observed ABI is audited |
| Pane-init terminated as PID namespace init | Explicit TERM/INT/HUP handlers forward to the PTY foreground process group |
| Orphaned descendants become zombies | SIGCHLD wakeups drain `waitpid(-1)` inside the PID namespace |
| Detached process survives shell exit | PID-1 cleanup sends TERM, waits the frozen grace period, then KILL |
| Shared namespace detached process survives shell exit | The shim signals only its shell group; worker `cgroup.kill` tears down the hard Pane scope |
| Control channel leaks into the target shell | The shim sets `FD_CLOEXEC` before spawning the shell; the real-PTY contract checks the FD is absent |
| Malformed or excessive control traffic | Versioned four-byte header, eight-byte payload cap, 1024-byte decoder cap, unknown-kind rejection, and signal whitelist |
| Arbitrary signal injection through the control channel | Only INT, TERM, HUP, and QUIT are accepted; KILL remains outside this channel |

The same-UID host process remains outside the threat model under ADR 0034.

## 5. Open risks

- `PR_SET_DUMPABLE=0` is reset by a later exec; it protects the helper window,
  not the target program.
- The current helper is Linux-focused but is packaged cross-platform as an
  internal binary. Non-Linux execution fails closed with exit code 125.
- Absolute symlinks inside protected paths are rejected. Users must use
  workspace-relative symlinks.
- Bubblewrap 0.5 lacks FD-pinned bind options. The source-path window between
  final host resolution and Bubblewrap mount setup remains residual risk.
- ADR 0034 terminal duties will materially increase this TCB.
- The Pane-init contract has local Linux PTY coverage and current x86_64
  Ubuntu, Debian, and Fedora coverage. aarch64 and the OEM distribution
  expansion remain environment/product gates.
- The Landlock adapter is custom syscall code, not a crate compatibility layer.
  It fails hard when a required ruleset operation is unsupported.

## 6. Fuzz and adversarial test plan

Before accepting terminal responsibilities:

- add `cargo-fuzz` targets for argv parsing with arbitrary option order,
  repeated options, NUL bytes, overlong paths, and trailing `--` combinations;
- fuzz path conversion and `openat2` wrappers with symlink trees, `..`,
  absolute symlinks, deep nesting, and mount boundaries;
- property-test Landlock mask generation for ABI 1 through the newest supported
  ABI, including unknown newer versions;
- fault-inject failures after no-new-privileges, after ruleset creation, after
  each path rule, after dumpability hardening, and immediately before exec;
- verify every injected failure exits 125 and never executes a sentinel target;
- run differential tests with Bubblewrap 0.5 and current releases;
- keep the implemented Pane-init control parser in the shared `cargo-fuzz`
  corpus; keep terminal signal and winsize coverage in real-PTY contract tests
  and avoid sleep-based assertions where event oracles are available.

## 7. Dependency policy

The init binary should keep direct dependencies limited to argument parsing,
the project runtime, and libc. Changes that add parsing frameworks, async
runtimes, dynamic plugin loading, or network-capable crates require explicit
review. CI should eventually enforce `cargo-deny` advisory/source policy and
`cargo-vet` review status for this binary and its transitive dependencies.

## 8. Future sandboxed-Pane controls

ADR 0034 assigns additional responsibilities to this TCB. The initial shim now
implements the terminal responsibilities below:

- explicitly handle SIGTERM, SIGINT, and SIGCHLD as PID namespace init;
- explicitly handle SIGHUP as PID namespace init;
- assign the spawned shell its own process group and make it the PTY foreground
  group;
- transfer a private control FD through the same-UID launcher broker, mark the
  shim copy close-on-exec, and accept only bounded, versioned control frames;
- leave window-size handling to qwed because PTY-master `TIOCSWINSZ` plus
  kernel `SIGWINCH` is sufficient; and
- terminate remaining namespace processes after shell exit.
- when PID namespace isolation is shared, report the host child PID through
  Bubblewrap status and rely on the worker-owned hard Pane cgroup for complete
  descendant teardown.

The following control remains required for nested credentials inside a
sandboxed Pane:

- use inherited descriptors or `SCM_RIGHTS` for command credentials and close
  the shim's copy after delivery.

The control-channel parser and message state machine are shared with the
`fuzz/fuzz_targets/sandbox_control.rs` harness. The restricted terminal-output
state machine has a separate `terminal_output` fuzz target. On 2026-11-16,
each target completed 10,000 local nightly runs without a crash or
counterexample.
