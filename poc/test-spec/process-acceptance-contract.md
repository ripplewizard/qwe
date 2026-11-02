# Process Acceptance POC Contract

Status: implementation in progress.

## History Operation

```text
TASK_HISTORY <request-id> <task-id>
```

Normal tasks create one TaskInstance row with `attempt=1`. After execution, the
row stores terminal state, exit code, start time, end time, and duration.

Response fields:

- `task_id`
- `attempts`
- ordered `instances`
- each instance contains `attempt`, `pid`, `pgid`, `start_token`, `state`,
  `exit_code`, `started_at`, `ended_at`, and `duration_ms`

## Signal Operation

```text
TASK_SIGNAL <request-id> <task-id> <SIGINT|SIGTERM|SIGKILL>
```

The signal is delivered to the task process group.

Expected exit conventions:

- SIGINT: 130
- SIGTERM: 143
- SIGKILL: 137

## Acceptance

- P01 stores exit code and duration.
- P03 records SIGINT, SIGTERM, and SIGKILL outcomes.
- P04 exposes a PGID and all tree processes belong to that group.
- P05 records non-zero child failure.
- P09 rejects a persisted PID whose start token does not match.
- P12 returns TaskInstance history after daemon restart.
