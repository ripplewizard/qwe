# Resource Monitoring POC Contract

Status: implemented and passing for Rust, Go, and C++.

## Operations

```text
TASK_RESOURCE <request-id> <task-id>
TASK_RESOURCE_SAMPLE <request-id> <task-id> <interval-ms> <samples>
```

## CPU and Memory

The POC reads process-group rows from `ps` and sums each member's lifetime
average `%CPU` and RSS. CPU is normalized in single-core units and may exceed
100% for a multi-process task.

RSS is reported as the sum of process RSS values in bytes. Shared pages may be
counted more than once; this limitation is returned with every result.

## GPU

The Linux POC attempts `nvidia-smi` raw GPU and compute-process queries. If the
tool or device is unavailable, the result is `supported=false` with a reason.
GPU metrics are attributed to a task only when a reported compute-process PID
belongs to the task process group.

## Acceptance

- RM01 returns process/group/task CPU values and the convention.
- RM02 returns process-count, RSS sum, and its limitation.
- RM03 records sampling latency for 100, 500, 1000, and 5000 ms intervals.
- RM04 reports supported raw GPU values or an explicit unsupported reason.
- RM05 never attributes GPU usage without PID-based task membership.
