# Stage 7: Linux Resource Governance

Status: Planned
Date: 2026-11-06

## Product Definition

Stage 7 makes QWE safer for long-running terminal Sessions by using Linux
cgroup v2 for tree-scoped resource accounting, enforcement, and visibility.
It extends the existing terminal/session runtime; it does not introduce a
container runtime.

The target positioning is:

> QWE is a lightweight terminal task runtime with tmux-style durable Sessions
> and Linux cgroup resource guardrails.

Desired characteristics:

- A Session keeps its PTY, terminal state, process tree, and attach lifecycle.
- Resource limits cover the complete Session process tree.
- qwed remains responsive when one Session exhausts memory, forks excessively,
  or consumes too much CPU.
- Unsupported Linux environments fail explicitly instead of silently ignoring
  a requested limit.
- Resource behavior and enforcement events are inspectable during long runs.

## Current Baseline

The V0 implementation already provides:

- One transient systemd user scope per Linux Session.
- Hard `memory.max`, `memory.swap.max`, `pids.max`, and `cpu.max` controls.
- `memory.high` reclaim/throttle threshold.
- `memory.oom.group=1` Session-level OOM behavior.
- Aggregate CPU-seconds monitoring with Session termination on violation.
- Dynamic policy updates on running Active or Held Sessions.
- SQLite persistence of the requested Pane resource policy.
- Current and peak memory/task metrics, CPU time, enforcement tier, and last
  violation in status output.
- OOM event retention before scope cleanup.
- Worker and cgroup lifecycle tests covering process exit, OOM, task fan-out,
  qwed restart, and residual scope cleanup.

See [ADR 0018](adr/0018-pane-resource-enforcement.md), the
[Technical Architecture](technical-architecture.md#10-resource-model), and
[Experiment 0018](experiments/0018-pane-resource-enforcement.md).

## Scope Boundary

This stage is about resource governance, not containerization.

Cgroup limits:

- bound CPU, memory, swap, and task count;
- apply to every descendant of a Session process tree;
- prevent one Session from exhausting the whole host;
- provide enforcement signals for diagnosis.

They do not provide:

- filesystem, network, PID, mount, or user namespaces;
- image and layer management;
- container network, volume, or port mapping;
- a security boundary against hostile code;
- Docker Compose, Kubernetes, or a container API.

Docker and Podman are used only as test environments or potential callers of
QWE. They are not the execution backend of a QWE Session.

## Workstreams

### R1. Resource Policy Productization

Priority: P0

- Define stable resource profiles such as `interactive`, `batch`, `tight`, and
  `unlimited`.
- Keep limits opt-in for existing users to avoid surprise terminations during
  upgrade.
- Allow explicit fields to override a selected profile.
- Report requested and effective policy separately.
- Define validation for combinations such as memory high versus maximum.
- Document how low limits interact with shell, compiler, and TUI workloads.

Exit:

- A user can create a Session from a documented profile.
- Unknown profiles, unsupported fields, and invalid limits fail with stable
  error codes.
- Existing Sessions retain their policy across qwed restart.

### R2. cgroup v2 Enforcement Hardening

Priority: P0

- Detect cgroup v2, systemd user manager, and individual controllers robustly.
- Verify `cpu`, `memory`, and `pids` controller availability before reporting
  hard enforcement.
- Make the Session startup/topology handshake reliable for very short-lived
  programs.
- Preserve the final Task PID/PGID and process-tree ownership contract.
- Audit scope cleanup after normal exit, OOM, forced termination, worker crash,
  qwed crash/restart, and systemd manager restart.
- Ensure failed policy application rolls back runtime and SQLite state.
- Prevent stale, duplicate, or unowned Session scopes from accumulating.
- Keep `qwe-session-*.scope` outside qwed's own service cgroup.

Exit:

- Controller absence returns `unsupported_capability`.
- Repeated OOM and task-count violations do not destabilize qwed.
- No residual Session scope remains after teardown or recovery tests.

### R3. Long-Run Visibility and Events

Priority: P0

- Expose current, peak, limit, and remaining resource values.
- Include `memory.events`, `pids.events`, and `cpu.stat` signals.
- Add PSI memory/CPU pressure where the kernel provides it.
- Record bounded events for OOM, fork rejection, CPU throttling, and policy
  updates.
- Distinguish a warning threshold from a hard limit.
- Keep event history bounded and retention explicit.
- Avoid high-frequency polling or unbounded metrics in SQLite.

Exit:

- `status` explains whether a Session is healthy, near a limit, or terminated
  by enforcement.
- Long-running resource data does not grow without a documented bound.

### R4. Long-Run Acceptance

Priority: P0

- Run 8 and 16 Session release-mode soak profiles.
- Keep Sessions alive through repeated attach/detach and input cycles.
- Exercise process churn, memory growth, CPU load, and task fan-out.
- Repeatedly restart qwed while Sessions and scopes remain live.
- Trigger memory, CPU, and task limits independently and concurrently.
- Verify qwed latency remains bounded while another Session is under pressure.
- Confirm no leaked workers, sockets, scopes, journals, or SQLite records.

Exit:

- A 24-hour qualification run passes on Fedora x86_64.
- A shorter deterministic soak is available for normal CI.

### R5. Linux Qualification Matrix

Priority: P1

Validate installed-systemd behavior in real VMs:

- Fedora x86_64
- Ubuntu 22.04 x86_64
- Ubuntu 24.04 x86_64
- Debian 12 x86_64

Each target must exercise systemd user service lifecycle, cgroup v2
delegation/controllers, package installation, limits, restart recovery, and
cleanup. A rootless container without systemd is not sufficient.

Exit:

- Only distributions that pass the complete resource/service gate are listed
  as supported.
- Experimental compatibility remains labeled separately.

### R6. Optional Resource Domains

Priority: P2

Evaluate one domain at a time after P0 exits:

- cgroup `io.max` for supported block devices;
- CPU weight/priority in addition to quota;
- filesystem quota or workspace size policy;
- aggregate user/workspace resource budgets;
- GPU observation or isolation only where a reliable platform contract exists.

No optional domain is silently approximated.

## Planned Test IDs

| ID | Case |
|---|---|
| RG-001 | Create Sessions from resource profiles and verify requested/effective policy |
| RG-002 | Reject missing cgroup controllers and systemd capabilities without fallback |
| RG-003 | Verify Session scope cleanup after exit, OOM, kill, and qwed/worker crashes |
| RG-004 | Keep qwed responsive while a Session exceeds memory, CPU, and task limits |
| RG-005 | Run installed-systemd resource gates on each claimed Linux distribution |
| RG-006 | Bound policy-event and metric history during a long-running soak |

## Design Decisions Required Before Implementation

1. Whether systemd user scopes are mandatory for hard Linux enforcement, or
   whether a separately delegated direct-cgroup path is worth supporting.
2. Whether resource profiles ship with defaults or are always selected
   explicitly.
3. Which enforcement events are durable versus exposed only through current
   status.
4. Whether I/O and CPU-weight controls belong in the first Stage 7 release or
   remain P2.

Recommended initial decision for the first decision:

- Require systemd user service plus cgroup v2 for supported hard limits.
- Keep Session limits opt-in.
- Make only bounded enforcement events durable.
- Defer I/O and CPU weight until the P0 kernel/resource lifecycle is stable.

## Non-Goals

- Implementing Docker or Podman as QWE's execution backend.
- Image building, registries, networks, mounts, port mapping, or Compose.
- Replacing tmux Window/Pane semantics in this stage.
- Claiming a security sandbox from cgroup limits alone.
- GPU isolation in the first delivery.
- Kubernetes, Mesh, or distributed scheduling.

## Stage Exit

Stage 7 is complete when:

- resource profiles and observable policy are stable;
- Linux cgroup lifecycle and failure paths pass;
- a 24-hour soak has no supervisor instability or leaked scopes;
- installed-systemd resource gates pass on every supported distribution;
- documentation clearly distinguishes resource governance from containers and
  security sandboxes.
