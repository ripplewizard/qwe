# Task Timeout POC Contract

Status: implemented and passing for Rust, Go, and C++.

## Operation

```text
TIMEOUT_START <request-id> <idempotency-key> <log-path> <timeout-ms>
              <program> [args...]
```

The client command is:

```text
qwe-poc task-start-timeout <socket> <key> <log> <timeout-ms>
        <program> [args...]
```

`timeout-ms` must be a positive integer.

## Deadline Behavior

1. Start the task in its own process group.
2. Persist the active task with state `running`.
3. At the deadline, send SIGTERM to the process group.
4. If the group remains alive after a short grace period, send SIGKILL.
5. Persist state `timed_out`.
6. Preserve process output and task identity metadata.

The POC deadline watcher is in memory. Surviving a daemon restart while a
deadline is pending belongs to the later terminal/task recovery work.

## Acceptance

- The task starts before the deadline.
- STATUS eventually reports `timed_out`.
- The whole process group is gone.
- SQLite stores `timed_out`.
- Output written before the deadline is retained.
- Elapsed time is consistent with the requested timeout.
