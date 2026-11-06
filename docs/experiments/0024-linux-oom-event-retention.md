# Experiment 0024: Linux OOM Event Retention

Status: Passed; Linux regression fixed
Date: 2026-11-06
Commit: `8d294df`
Environment: Fedora Linux 35 x86_64

## Failure

An interactive `just stage4-check` run observed:

```text
memory_status["resource_metrics"]["last_violation"] == "memory_max"
AssertionError: ... "last_violation": null
```

The kernel had already killed the Session because its cgroup memory limit was
exceeded, but the final status no longer retained the enforcement reason.

## Root Cause

The transient scope was started with `systemd-run --collect`. When OOM killed
the Session, systemd immediately garbage-collected the failed scope and its
`memory.events` file. The worker's final sample could still read residual
counters, but the cgroup violation event was already gone.

## Fix

- Removed `--collect` so failed scopes remain available long enough to sample
  `memory.events`.
- Capture the final sample before the worker exposes the terminal-exited state.
- Move cgroup cleanup to the monitor thread after its final observation.
- Explicitly `systemctl --user reset-failed` the scope during worker cleanup.
- Fall back to directory removal for non-systemd cgroups.

## Verification

The focused Stage 4 capability probe passed five consecutive runs:

```text
run=1 ... resource_capability="pass"
run=2 ... resource_capability="pass"
run=3 ... resource_capability="pass"
run=4 ... resource_capability="pass"
run=5 ... resource_capability="pass"
remaining_scopes=
```

The full `just stage4-check` then passed, including credential delivery,
resource limits, forced teardown, service lifecycle, idempotency, and second
UID rejection.

The Linux `just release-gate` also passed:

```json
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":552,"soak_seconds":15.0,"p50_ms":8.384,"p95_ms":26.14,"p99_ms":26.504,"max_ms":26.809,"max_rss_bytes":153911296}
```

Generated Linux archive SHA-256:
`7835ddc5a23e8c85be88b01051b6415493d34b6b09b5231084ea3e704cc681bf`.
