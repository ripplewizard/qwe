# Experiment 0018: Pane Resource Enforcement and Dynamic Limits

Status: Passed on macOS ARM64 and Fedora Linux x86_64
Date: 2026-11-05

## Scope

This experiment validates ADR 0018:

- Linux hard cgroup v2 limits for a complete Session tree.
- macOS current/peak resource observation and monitored termination.
- Aggregate CPU-time enforcement.
- Dynamic policy updates on a running Held Session.

## Linux

Environment:

- Fedora Linux 35 x86_64.
- cgroup v2 with `cpu`, `memory`, `pids`, and `cpuset` controllers.
- systemd 249 user manager.

Each Session was started in a transient `systemd --user` scope. The launcher
reported its final PID and cgroup path before exec, preserving QWE's task
PID/PGID and terminal job-control semantics.

Observed:

```text
memory.max      = 67108864
memory.swap.max = 0
pids.max        = 5
cpu.max         = dynamic via systemd property
memory.oom.group = 1
```

- A 128 MiB allocation under a 64 MiB memory max was killed by cgroup OOM.
- A fork fan-out above `tasks_max` failed and the Session exited non-zero.
- Dynamic update changed memory, task, and CPU-quota policy on a running
  Session and persisted the same policy in SQLite.
- `memory.peak` and `pids.peak` are unavailable on this kernel; QWE falls back
  to its sampled high-water mark.

## macOS

Environment:

- macOS 26.5.1 arm64.

The worker used public libproc APIs:

- `proc_listchildpids` for the task tree.
- `proc_pid_rusage` for physical footprint and CPU accounting.

Observed:

- A 128 MiB allocation under a 64 MiB monitored limit was terminated with
  `last_violation=memory_max`.
- A fork fan-out above `tasks_max` was terminated with
  `last_violation=tasks_max`.
- A busy loop above `cpu_seconds=1` was terminated with
  `last_violation=cpu_seconds`.
- Current/peak memory and task metrics were returned through Session status.
- Dynamic Held-Session updates applied without restart.

macOS remains explicitly `monitored`: the 100 ms sampling interval permits
bounded overshoot and is not described as a hard cgroup-equivalent quota.

## Gate

`just stage4-capabilities` returned:

```json
{"case":"CRED-001/RES-001/RES-003/RES-004/RES-005/RES-006/SEC-001/KILL-001","credential_descriptor":"pass","credential_redaction":"pass","resource_capability":"pass","malformed_frame":"pass","oversized_input":"pass","session_teardown":"pass","batch_task_teardown":"pass","server_teardown":"pass"}
```

The service crash-restart and graceful-stop gates also continue to pass with
per-Session scopes outside the qwed service cgroup.
