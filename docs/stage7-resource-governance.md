# Stage 7: Linux Resource Governance

Status: In progress; shared prerequisites and deterministic P0 gates implemented
Date: 2026-11-10

## Product Definition

Stage 7 makes QWE safer for long-running terminal Sessions and Panes by using
Linux cgroup v2 for tree-scoped resource accounting, enforcement, and
visibility. It extends the existing terminal runtime; it does not introduce a
container runtime.

The target positioning is:

> QWE is a lightweight terminal task runtime with tmux-style durable Sessions
> and Linux cgroup resource guardrails.

Desired characteristics:

- A Session groups one or more Pane workers.
- Each Pane keeps its PTY, terminal state, process tree, and attach lifecycle.
- Resource limits cover the complete Pane process tree and support Session and
  Task targets through the same ResourceScope interface.
- qwed remains responsive when one Pane exhausts memory, forks excessively, or
  consumes too much CPU.
- Unsupported Linux environments fail explicitly instead of silently ignoring
  a requested limit.
- Resource behavior and enforcement events are inspectable during long runs.

## Current Baseline

The implemented baseline provides:

- One transient systemd user scope per Linux Pane.
- Hard `memory.max`, `memory.swap.max`, `pids.max`, and `cpu.max` controls.
- `memory.high` reclaim/throttle threshold.
- `memory.oom.group=1` Pane-level OOM behavior.
- Aggregate CPU-seconds monitoring with Pane termination on violation.
- Dynamic policy updates on running Active or Held Sessions.
- SQLite persistence of the requested Pane resource policy.
- Current and peak memory/task metrics, CPU time, enforcement tier, and last
  violation in status output.
- OOM event retention before scope cleanup.
- Worker and cgroup lifecycle tests covering process exit, OOM, task fan-out,
  qwed restart, and residual scope cleanup.

Stage 6.4 validated a disposable one-worker-per-Pane topology and distinct
cgroup v2 scopes in [Experiment 0028](experiments/0028-stage6-shared-prerequisites-spike.md).
[ADR 0021](adr/0021-shared-prerequisites.md) accepts Pane-first enforcement.
Production Pane-worker inventory, Pane-addressed control routing, schema v11,
and multiple Panes per Session are implemented. The qwed-owned observer
projection remains open.

See [ADR 0018](adr/0018-pane-resource-enforcement.md), the
[Technical Architecture](technical-architecture.md#10-resource-model), and
[Experiment 0018](experiments/0018-pane-resource-enforcement.md).

## Implementation Slices 1-2

The first Stage 7 slice implements the deterministic P0 foundation:

- Explicit `interactive`, `batch`, `tight`, and `unlimited` profiles.
- `resource_profile` selection with `resource_policy` field overrides.
- Stable `unknown_resource_profile`, `unsupported_resource_field`, and
  `invalid_resource_policy` errors.
- Per-controller Linux capability reporting and validation. Missing cgroup v2,
  systemd user service, `cpu`, `memory`, or `pids` capability rejects the
  affected limit with `unsupported_capability`.
- Pane UUID-derived `qwe-pane-<pane-id>.scope` units, including teardown of
  detached process sessions through the complete scope.
- A topology acknowledgement blocks the user command until the worker has
  installed the cgroup event watch and applied the initial policy, so
  short-lived programs still have a reliable Pane ownership handshake.
- Separate requested and effective policy status, current/peak/remaining
  values, controller counters, PSI, health classification, and a 32-event
  in-memory retention bound per Pane worker.
- Linux cgroup event watches capture OOM, task-limit, and CPU-throttle changes
  from transient scopes before systemd removes their control files.
- `just stage7-check`, including a real-process probe for profiles, policy
  errors, dynamic updates, qwed crash/restart recovery, bounded events, and
  detached-job scope cleanup.
- Schema v11 with stable v10 backup, Pane-worker, ResourceScope, Principal, and
  Pane-keyed Task/attachment records.
- Production multi-Pane inventory and recovery keyed by Pane, with
  `PANE_CREATE`, `PANE_STATUS`, `PANE_ATTACH`, `PANE_CLOSE`, and `PANE_KILL`.
- A real multi-Pane probe proving qwed adopts all Pane workers after SIGKILL
  and that one worker crash does not mark unrelated Panes lost.
- RG-004 concurrent memory, CPU-seconds, and pids.max pressure while 350+
  qwed PING samples remain far below the 1-second p95 threshold.
- A deterministic 8-Session x 2-Pane short soak with repeated qwed restarts,
  Attach/detach, process churn, SQLite persistence, and scope cleanup.
- `just stage7-qualification-node` and `just stage7-qualification-matrix` for
  installed-systemd VM gates.

The deterministic probes exercise RG-001 through RG-004 and RG-006. Local
Fedora installed-systemd qualification passes. Ubuntu 22.04, Ubuntu 24.04, and
Debian 12 qualification are blocked until real-VM targets are available; no
container result is accepted as a substitute. The bounded five-hour soak remains open
until its release-mode run finishes. Results and environment details are in
[Experiment 0029](experiments/0029-stage7-multipane-pressure-and-qualification.md).

### Resource Profiles

Profiles are opt-in shorthands. The resolved policy is persisted; the profile
name is not a second authority. Status classifies an exact policy match as the
named profile and any override combination as `custom`. `unlimited` requests
no limits, but explicit fields may still be supplied.

| Profile | `memory.high` | `memory.max` | `swap.max` | `pids.max` | CPU quota |
|---|---:|---:|---:|---:|---:|
| `interactive` | 1 GiB | 2 GiB | 0 | 256 | 200% |
| `batch` | 2 GiB | 4 GiB | 1 GiB | 512 | 400% |
| `tight` | 192 MiB | 256 MiB | 0 | 64 | 100% |
| `unlimited` | none | none | none | none | none |

Memory limits are intentionally conservative for shell utilities and small
coding agents, not compilers or large language-server processes. `batch`
provides the broader default. Lowering `memory.max` without explicitly setting
`memory.high` clamps the inherited high threshold to the new maximum.

`qwe session create` and `qwe session limits` accept `--profile`. The control
API accepts `resource_profile` plus an optional partial `resource_policy`
object for explicit overrides.

### Requested and Effective Policy

`requested_resource_policy` is the persisted qwed authority. The worker reads
the applied cgroup files into `effective_resource_policy`; the worker retains
the last effective value through final sampling and scope cleanup.

`resource_metrics` reports:

- `health`: `healthy`, `near_limit`, `pressure`, or `enforced`;
- current, peak, limit, and remaining memory/task values;
- `memory.events`, `pids.events`, and `cpu.stat` counters;
- memory and CPU PSI where available;
- bounded `policy_update`, OOM, fork-limit, and CPU-throttle events.

Resource events are not durable. Each Pane worker retains at most 32 events in
memory and reports that retention policy explicitly. A worker replacement
starts a new bounded event history.

## Scope Boundary

This stage is about resource governance, not containerization.

Cgroup limits:

- bound CPU, memory, swap, and task count;
- apply to every descendant of a Pane process tree;
- prevent one Pane from exhausting the whole host;
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
- Store requested policy against the ResourceScope target; the first delivery
  creates Pane scopes with a logical Session parent.
- Keep limits opt-in for existing users to avoid surprise terminations during
  upgrade.
- Allow explicit fields to override a selected profile.
- Report requested and effective policy separately.
- Define validation for combinations such as memory high versus maximum.
- Document how low limits interact with shell, compiler, and TUI workloads.

Exit:

- A user can create a Pane from a documented profile.
- Unknown profiles, unsupported fields, and invalid limits fail with stable
  error codes.
- Existing Sessions retain their policy across qwed restart.

### R2. cgroup v2 Enforcement Hardening

Priority: P0

- Detect cgroup v2, systemd user manager, and individual controllers robustly.
- Verify `cpu`, `memory`, and `pids` controller availability before reporting
  hard enforcement.
- Make the Pane startup/topology handshake reliable for very short-lived
  programs.
- Preserve the final Task PID/PGID and process-tree ownership contract.
- Audit scope cleanup after normal exit, OOM, forced termination, worker crash,
  qwed crash/restart, and systemd manager restart.
- Ensure failed policy application rolls back runtime and SQLite state.
- Prevent stale, duplicate, or unowned Pane scopes from accumulating.
- Keep Pane scopes outside qwed's own service cgroup and make their names
  derivable from stable Pane identity.
- Tear down detached interactive job groups as part of the complete Pane
  scope; process-group signaling alone is insufficient.

Exit:

- Controller absence returns `unsupported_capability`.
- Repeated OOM and task-count violations do not destabilize qwed.
- No residual Pane scope remains after teardown or recovery tests.

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

- `status` explains whether a Pane is healthy, near a limit, or terminated
  by enforcement.
- Long-running resource data does not grow without a documented bound.

### R4. Long-Run Acceptance

Priority: P0

- Run 8 and 16 Session release-mode soak profiles with multiple Panes where
  the qualification profile requires them.
- Keep Panes alive through repeated attach/detach, resize, input, and snapshot
  cycles.
- Exercise control inventory, task status/history, Lease status/touch, and
  resource-policy updates during the same run.
- Exercise process churn, memory growth, CPU load, and task fan-out.
- Repeatedly restart qwed while Sessions and scopes remain live.
- Trigger memory, CPU, and task limits independently and concurrently.
- Verify qwed latency remains bounded while another Pane is under pressure.
- Confirm no leaked workers, sockets, scopes, journals, or SQLite records.
- Persist qwed logs and periodic progress records outside volatile `/tmp`.

Exit:

- A five-hour operation-dense qualification run passes on Fedora x86_64.
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
| RG-001 | Create Panes from resource profiles and verify requested/effective policy |
| RG-002 | Reject missing cgroup controllers and systemd capabilities without fallback |
| RG-003 | Verify Pane scope cleanup after exit, OOM, kill, detached jobs, and qwed/worker crashes |
| RG-004 | Keep qwed responsive while a Pane exceeds memory, CPU, and task limits |
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
- Use Pane-level hard scopes with a logical Session parent; do not retain the
  V0 per-Session limit as the long-term enforcement target.
- Keep Pane limits opt-in.
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

- Pane-level ResourceScope profiles and observable policy are stable;
- Linux cgroup lifecycle and failure paths pass;
- a five-hour soak has no supervisor instability or leaked scopes;
- installed-systemd resource gates pass on every supported distribution;
- documentation clearly distinguishes resource governance from containers and
  security sandboxes.
