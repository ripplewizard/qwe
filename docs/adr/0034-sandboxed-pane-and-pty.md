# ADR 0034: Sandboxed Pane, Interactive PTY, and Path Trust

Status: Accepted for the next sandbox milestone; implementation not started
Date: 2026-11-15
Extends: [ADR 0032](0032-agent-execution-model.md),
[ADR 0033](0033-bubblewrap-sandbox-adapter.md)

## Context

The Linux `AGENT_EXEC` Bubblewrap slice is implemented and qualified. A
long-lived sandboxed Pane and interactive Agent PTY share terminal machinery but
have different authority. They must not be implemented by extending the
pipe-split path without defining controlling terminal, signal, resize, nested
policy, credential, and terminal-output behavior.

This ADR freezes the next-milestone decisions. A decision marked **adopted** is
binding. A **PoC** decision records only the direction and validation method
until an experiment supplies the missing kernel or Bubblewrap facts. A
**product** decision remains open and blocks implementation where noted.

## Summary Decisions

| ID | Decision | Status | Summary |
|---|---|---|---|
| D1 | Sandboxed Pane Bubblewrap capability | Adopted; P1 pass | `AGENT_EXEC` keeps 0.5 compatibility; sandboxed Pane requires FD-binding features detected at runtime; first observed upstream support is 0.10.0 |
| D2 | Capability split | Adopted; scaffold implemented | Separate `sandboxed_pane`, `pane_exec`, `pane_input`, and `pane_resize`; target-type enforcement activates with Pane sandbox metadata |
| D3 | Terminal sequence policy | Adopted | Restricted output uses a display-only whitelist; host-sensitive control and query sequences are dropped |
| D4 | Input audit | Adopted | Metadata only by default; no content hashes; no content when PTY ECHO is off |
| D5 | Landlock | Adopted | Profile field `off | best_effort | required{min_abi}`; sandboxed Pane defaults to required |
| D8 | `pane_input` authority | Adopted | Treated as arbitrary code execution in the target Pane |
| D9 | Target-type grading | Adopted | Sandboxed Pane input is ordinary authority; unsandboxed Pane input is separately high-trust and denied by default |
| D14 | Nested profile ordering | Adopted | Stackable fields may only tighten; fixed fields must equal the parent |
| D15 | Nested enforcement mechanism | Adopted after P2 | Use shim-stacked Landlock, seccomp, and no-new-privileges; do not use nested Bubblewrap as the refinement mechanism |
| D32 | Pane credentials | Adopted | Long-lived Pane holds no FD3; credentials remain single-command |
| D38 | Sandbox-originated path influence | Adopted | Sandbox Agent influence through workspace is in scope even though same-UID host processes are excluded |

## Decision Register

### A. Blocking decisions

- **D1 adopted direction:** support two Bubblewrap capability tiers. Existing
  `AGENT_EXEC` continues to support 0.5. Sandboxed Pane requires FD-pinned bind
  support (`--bind-fd`/`--ro-bind-fd` or equivalent). Detect capabilities with a
  harmless probe; never infer from a version string. Cache the result. Missing
  features fail `sandbox_backend_unavailable` with the missing feature named.
  P1 found upstream 0.10.0 as the first observed release with FD binding.
  Runtime qualification uses a help check plus a harmless FD mount; the result
  is stored as `sandbox_audit.fd_bind`.
- **D2 adopted:** see capability model below.
- **D3 adopted:** restricted terminal output uses a whitelist, not an OSC 52
  blacklist. Filtering occurs before every Attach client and is defined in
  [Terminal Output Policy](../terminal-output-policy.md).
- **D4 adopted:** input audit defaults to length, time, target, Principal,
  effective Landlock level, and profile version. Content requires explicit
  opt-in. No content or hash is recorded when terminal ECHO is disabled.
- **D5 adopted:** add profile field
  `landlock: off | best_effort | required{min_abi}`. `AGENT_EXEC` retains an
  optional default; sandboxed Pane defaults to `required{min_abi}` where
  `min_abi` covers the filesystem rules, not network ABI. An administrator may
  explicitly downgrade to `best_effort`; effective enforcement is queryable and
  audited. Silent downgrade is forbidden.

### B. Capability model

- **D6 adopted; operation/scope scaffold implemented:** `sandboxed_pane`
  authorizes only creation of a sandboxed
  Pane. It grants no operation on an existing Pane.
- **D7 adopted; operation/scope scaffold implemented:** `pane_exec` authorizes
  injecting an `AGENT_EXEC` request into
  an existing Pane and is constrained by D14 refinement.
- **D8 adopted; high-trust issuer floor implemented:** `pane_input` is
  equivalent to arbitrary code execution in the
  target Pane. Authorization and UI text must state this explicitly.
- **D9 adopted:** input to a sandboxed Pane is ordinary delegated authority.
  Input to an unsandboxed Pane is a separate high-trust capability and is not
  issued by default. Until Pane sandbox metadata exists, all `pane_input`
  issuance requires a high-trust management Principal.
- **D10 adopted; operation/scope scaffold implemented:** `pane_resize` is
  separate from `pane_input`; it still carries terminal-control authority.
- **D11 adopted:** capability delegation is monotonic. A holder cannot issue a
  broader capability than its own authority.
- **D12 adopted:** capability expiry and ordinary revocation do not terminate an
  existing Pane. The Principal loses operation authority immediately.
  - Revocation carries `terminate: false` by default. High-risk profiles,
    including `network:inherit`, force `terminate: true`.
  - Revocation invalidates queued input and output operations before returning;
    buffered bytes from a revoked Principal cannot be written to the PTY.
  - An orphaned sandboxed Pane has no valid operation capability and no human
    attachment. Default idle TTL is 15 minutes, with at most 16 orphaned
    sandboxed Panes per qwed. Exceeding the limit rejects new Pane creation.
    These values are frozen as `SANDBOXED_PANE_ORPHAN_TTL_MS` and
    `MAX_ORPHANED_SANDBOXED_PANES`.
  - A separate high-trust administrative teardown path does not depend on a
    revoked capability.

### C. Profile and nesting

- **D13 adopted; schema implemented:** profile fields are classified as
  stackable or fixed.
  - Stackable: filesystem visibility/write scope, syscall set, network socket
    creation, Landlock rules.
  - Fixed: mount layout, PID/IPC namespace, private `/tmp`, hidden HOME, and the
    protected-path set.
- **D14 adopted; core type implemented:** stackable fields may only tighten.
  Fixed fields must equal the parent. `RefinedSandboxProfile` performs
  construction-time validation, not only command-dispatch checks.
- **D15 adopted after P2:** strict seccomp alone permits nested Bubblewrap, but
  once Landlock is applied the inner Bubblewrap cannot initialize its uid map.
  Refinement therefore uses shim-stacked Landlock, seccomp, and
  `no_new_privs`, which remain monotonic at the kernel layer.
- **D16 adopted after P2:** without a new network namespace, refinement can
  deny new `socket(2)` calls but cannot invalidate inherited socket FDs.
  Existing sockets remain usable and this weaker semantic is explicit.
- **D17 adopted:** profiles remain versioned and deny unknown fields. Older
  readers reject unknown enum variants with `sandbox_profile_unsupported`.

### D. `qwe-sandbox-init` shim

- **D18 adopted; initial shim implemented:** the shim reuses an inherited
  controlling terminal and only attempts `setsid`/`TIOCSCTTY` when none exists.
  It handles signal forwarding, child reaping, and stacked policy for a
  sandboxed Pane. It does not forward winsize. Ordinary `AGENT_EXEC` keeps its
  current non-terminal path.
- **D19 resolved by P3:** `TIOCSWINSZ` on the qwed-owned PTY master delivers
  SIGWINCH to the foreground process group without shim involvement.
- **D20 adopted; P4 pass and initial shim implemented:** when the shim is PID
  namespace init, it explicitly handles SIGTERM, SIGINT, SIGHUP, and SIGCHLD.
  Default-ignored signals are not
  acceptable.
- **D21 adopted:** the shim applies no-new-privileges, dumpability suppression,
  and seccomp denial of ptrace. It never passes its control-channel FD to the
  target shell.
- **D22 adopted:** the shim control channel has a version, bounded messages, and
  unknown-message failure. It is inaccessible from inside the sandbox and is a
  fuzz target.
- **D23 adopted:** seccomp/Landlock/control-channel setup failure terminates
  before target exec and is surfaced as `sandbox_setup_failed`.
- **D24 decided:** replacing the shell with `exec bash`, `exec zsh`, or another
  program does not change the shim or effective policy.
- **D25 resolved:** ordinary Panes transition to `exited`, retain final output,
  terminal status, and identity until explicit `SESSION_CLOSE`; they do not
  auto-restart. Sandboxed Pane follows the same observable lifecycle.
  - Shell exit stops input acceptance and starts sandbox-tree teardown.
  - The shim sends SIGTERM to remaining namespace processes, waits a bounded
    five-second grace period, then sends SIGKILL.
    `SANDBOXED_PANE_SHUTDOWN_GRACE_MS` freezes the grace period.
  - This intentionally differs from an ordinary Pane where detached background
    processes may survive. Work that must outlive the shell uses `AGENT_EXEC`.
  - Exit records identify normal exit, signal, `terminate`, revocation, TTL, or
    teardown cause.

### E. Terminal and PTY

- **D26 adopted:** restricted output permits display-oriented sequences and
  drops other sequences by default. Policy is shared by all human Attach
  clients.
- **D27 adopted:** terminal query responses are disabled by default. CSI 6n,
  CSI 21t, DECRQSS, and OSC 10/11 are covered as output-to-input injection
  risks; any enabled response has type and length bounds.
- **D28 adopted:** title, hyperlink, notification, clipboard, and similar OSC
  sequences are disabled for restricted output unless an explicit policy allows
  them.
- **D29 adopted:** input queues are bounded. Overload returns an explicit error
  without blocking qwed's event loop.
- **D30 adopted:** resize and output/input events share a deterministic ordered
  stream for replay and debugging.
- **D31 adopted:** without `pane_input`, PTY writes are rejected.

### F. Credentials and audit

- **D32 adopted:** a long-lived Pane never retains FD3.
- **D33 adopted:** nested `AGENT_EXEC` credentials use inherited descriptors or
  `SCM_RIGHTS`, never filesystem paths or Pane-visible sockets. The shim closes
  its copy after delivery.
- **D34 adopted:** `network:inherit` does not provide credential
  confidentiality.
- **D35 adopted:** audit includes target, Principal, input length/time, profile
  version, effective Landlock level, and policy decisions.
- **D36 adopted:** content audit is explicit opt-in. ECHO-off input records
  neither content nor a hash.

### G. Threat model

- **D37 decided:** same-UID host processes remain outside the threat model.
- **D38 adopted:** sandbox-originated workspace content is untrusted when it can
  influence later qwed/worker host-path resolution.
- **D39 implemented:** qwed and worker paths use
  `openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)`. Absolute symlinks fail
  closed; relative symlinks must remain inside the workspace.
- **D40 adopted dependency:** sandboxed Pane requires FD-pinned mount sources as
  part of D1. Until then, local `openat2` does not close the concurrent sandbox
  Agent path-replacement window.
- **D41 adopted:** protected paths remain read-only in a sandboxed Pane,
  including files whose later host-side execution would change trust.
- **D42 decided:** Landlock-only is never reported as Bubblewrap.
- **D43 decided:** untrusted multi-tenancy remains out of scope.

### H. Seccomp

- **D44 P6 pass for current strict:** the existing denylist passes target
  glibc/Python/Rust/shell probes. Each future allowlist or broader named
  profile still requires its own target-runtime matrix.
- **D45 adopted:** denied-syscall diagnosis may use TRAP/LOG only through a
  compile-time or administrator-local debug facility. It cannot be selected by
  capability or profile.
- **D46 adopted:** x86_64 and aarch64 profiles and tests remain separate.

### I. Deployment

- **D47 decided:** container success is not a product requirement. Containers
  normally fail closed and user documentation names the reason code.
- **D48 policy adopted; target scope pending product:** the matrix has separate
  `AGENT_EXEC` and `sandboxed_pane` capability columns and is split by CPU
  architecture. Tier 1 currently contains Ubuntu 22.04, Ubuntu 24.04 AppArmor,
  Debian 12, and Fedora 44 SELinux enforcing. Tier 2 evaluation order is
  openEuler, Kylin, then Rocky. No distribution is publicly claimed as
  sandboxed-Pane supported before its evidence is archived.
- **D49 decided:** macOS remains Linux-only sandbox unsupported.
- **D50 decided:** VM, OCI, and external providers require separate ADRs.

### J. Deferred

- **D51:** `shell_exec` waits until sandboxed Pane and interactive PTY authority
  stabilize.
- **D52:** cross-Pane orchestration and multi-writer commands require an
  independent authority design.
- **D53:** persistent semantic history, environment mutation, and host
  management remain deferred.

## PoC Gates

| ID | Question | Decisions |
|---|---|---|
| P1 | First Bubblewrap release with FD binding; exact feature probe | D1, D40 |
| P2 | Nested Bubblewrap feasibility; sufficiency of Landlock/seccomp stacking | D15/D16 resolved; pass |
| P3 | SIGWINCH delivery and `tcgetpgrp` under PID namespace | D18, D19; pass |
| P4 | Shim PID-1 signal and child-reaping behavior | D20; pass |
| P5 | Target-distribution Landlock ABI distribution | D5, D48; pass |
| P6 | Named seccomp runtime compatibility | D44; current strict passed |

P1-P6 evidence is recorded in
[Experiment 0051](../experiments/0051-sandbox-pane-poc-gates.md).

## Implementation Order

1. Freeze the adopted decision register and terminal output policy.
2. P1-P6 are complete; nested refinement, shim behavior, Landlock distribution,
   and current strict runtime compatibility are fixed.
3. Add `landlock` to the typed profile and enforce effective-level audit.
4. Implement sandboxed Pane capability, shim, lifecycle, and nested refinement.
5. Implement `pane_input`, `pane_resize`, bounded audit, and terminal filtering.
6. Resolve D12, D25, and D48 before their dependent behavior ships.

## Consequences

- `AGENT_EXEC` remains available on Bubblewrap 0.5, while sandboxed Pane has a
  higher feature floor.
- `pane_input` is explicitly documented as code execution authority.
- The shim becomes a focused TCB with fuzzing and dependency review.
- Restricted terminal output trades some terminal features for resistance to
  output-to-input injection.
- D12 and D25 no longer block protocol scaffolding or Pane-exit/revocation
  implementation. D48 target scope remains a product input to the public
  distribution matrix.

## References

- [ADR 0032](0032-agent-execution-model.md)
- [ADR 0033](0033-bubblewrap-sandbox-adapter.md)
- [Terminal Output Policy](../terminal-output-policy.md)
- [`qwe-sandbox-init` Threat Model](../sandbox-init-threat-model.md)
- [Linux openat2](https://man7.org/linux/man-pages/man2/openat2.2.html)
