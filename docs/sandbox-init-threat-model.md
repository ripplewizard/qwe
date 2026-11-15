# `qwe-sandbox-init` Threat Model and Fuzz Plan

Date: 2026-11-15
Status: Current for `AGENT_EXEC`; expanded scope planned by ADR 0034

## 1. Role in the trust boundary

`qwe-sandbox-init` executes inside mandatory Bubblewrap confinement, immediately
before the requested command. It is trusted computing base code because it:

- receives worker-generated profile arguments;
- applies `PR_SET_NO_NEW_PRIVS` and Landlock;
- disables its own process dumpability;
- exports the successful Landlock ABI; and
- replaces itself with the requested program.

It does not load the Bubblewrap seccomp program. Bubblewrap applies seccomp
before executing the init helper. Any future terminal/signal behavior added for
ADR 0034 expands this trusted computing base and requires a new review.

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
- fuzz signal/winsize forwarding deterministically only after ADR 0034 terminal
  code exists; avoid sleep-based race tests.

## 7. Dependency policy

The init binary should keep direct dependencies limited to argument parsing,
the project runtime, and libc. Changes that add parsing frameworks, async
runtimes, dynamic plugin loading, or network-capable crates require explicit
review. CI should eventually enforce `cargo-deny` advisory/source policy and
`cargo-vet` review status for this binary and its transitive dependencies.
