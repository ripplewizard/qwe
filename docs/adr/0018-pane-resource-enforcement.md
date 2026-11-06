# ADR 0018: Pane Resource Enforcement and Dynamic Limits

Status: Accepted
Date: 2026-11-05

## Context

QWE protects long-running terminal Sessions from resource leaks and runaway
process trees. It is not a container security boundary. ADR 0017 established
initial per-process resource controls and explicit unsupported capabilities,
but a per-process limit does not enforce a shared Pane budget.

Linux cgroup v2 provides kernel-enforced tree-level memory, task-count, and CPU
bandwidth controls. macOS has no equivalent public, unprivileged primitive.
Public libproc APIs can observe the process tree, but monitoring cannot provide
a hard quota without a polling gap.

## Decision

### Policy

A Pane policy has optional limits for:

- `memory_high_bytes`: reclaim/throttle threshold.
- `memory_max_bytes`: hard memory ceiling.
- `memory_swap_max_bytes`: swap ceiling, defaulting to zero when a memory
  hard limit is configured.
- `tasks_max`: maximum processes plus threads in the tree.
- `cpu_quota_percent`: aggregate CPU bandwidth, where 100% is one CPU.
- `cpu_seconds`: aggregate consumed CPU time.

Requested policy is durable Pane state. `CONTROL_STATUS` reports requested and
effective values, enforcement tier, current usage, peak usage, and enforcement
events.

### Linux

- Enforcement uses cgroup v2 through transient systemd user scopes.
- qwed and its workers remain outside Session resource scopes. Every Session
  runs in its own `qwe-session-*.scope`, so a qwed crash/restart does not make
  systemd reattach to a controller-enabled service cgroup containing old
  workers.
- `qwe-session-exec` reports its final PID and scope cgroup path to the worker
  before exec. The target keeps the existing PID/PGID contract, and every
  descendant inherits the Pane budget.
- `memory.max`, `memory.swap.max`, `pids.max`, and `cpu.max` implement hard
  limits. `memory.oom.group=1` makes memory exhaustion terminate the Session
  workload as a unit.
- Aggregate `cpu_seconds` uses a bounded monitor over `cpu.stat` and terminates
  the Session process group when consumed CPU time crosses the limit.
- If the systemd user manager or a required controller is unavailable, the affected
  capability is `unsupported`; QWE does not silently substitute per-process
  limits.

### macOS

- Public libproc APIs provide tree observation through `proc_listchildpids` and
  `proc_pid_rusage`.
- The worker samples aggregate physical footprint, task count, and CPU time,
  retaining current and peak values.
- Configured memory, task-count, and cumulative CPU limits are enforced by
  terminating the Session process group after a breach.
- This is `monitored` enforcement. A bounded polling gap means overshoot is
  possible; it is never reported as a hard cgroup-equivalent limit.
- Per-process physical footprints are summed. Shared pages can therefore make
  the macOS aggregate an estimate rather than an exact charge.
- Per-process `taskpolicy`/jetsam limits do not represent a tree budget and are
  not used as evidence of Pane-level support.

### Dynamic updates

- `SESSION_RESOURCE_UPDATE` replaces requested policy at runtime and requires
  Active or Held Session authority.
- Raising limits is immediate.
- Lowering a Linux memory hard limit may immediately trigger reclaim/OOM.
- If `memory_max_bytes` is lowered without explicitly changing
  `memory_high_bytes`, the existing high threshold is clamped to the new max.
  Explicitly conflicting high/max values are rejected.
- Lowering `tasks_max` below current usage does not kill existing tasks; it
  prevents new forks until usage falls below the new maximum.
- Monitor-based macOS changes take effect on the next sample.
- Policy changes are persisted before the operation is reported complete. If
  runtime application fails, QWE attempts to restore the prior durable policy.
- Changes are generation-ordered through the existing request idempotency and
  attachment fencing model.

## Consequences

- Linux hard Pane limits require an available systemd user manager; otherwise
  the hard capability is reported unsupported.
- macOS provides honest observation and monitored enforcement, not hard
  isolation.
- Dynamic limits can cause immediate task termination when lowered below
  current usage; callers must surface that behavior.
- The canonical policy's `tasks_max` counts threads on Linux, so UI and docs
  must not describe it as a process-only counter.
- Resource enforcement remains same-UID fault containment, not a multi-tenant
  security boundary.
