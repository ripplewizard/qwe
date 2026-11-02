# Runtime POC Contract

This contract is limited to POC comparison. It is not the production QWE CLI.

## Processes

- `qwe-poc daemon <socket-path> <sqlite-path>` runs the local daemon.
- `qwe-poc task-start <socket> <key> <log> <program> [args...]` creates or
  returns the TaskInstance for an idempotency key.
- `qwe-poc task-status <socket> <task-id>` queries one persisted TaskInstance.
- `qwe-poc task-stop <socket> <task-id>` terminates the task process group.

Every client prints one JSON response. The request ID is echoed in every
response. Errors use `ok=false`, a stable `error` code, and a human-readable
message.

## IPC

The daemon listens on a Unix domain socket and processes one tab-delimited
request per connection:

```text
START  <request-id> <idempotency-key> <log-path> <program> [args...]
STATUS <request-id> <task-id>
STOP   <request-id> <task-id>
```

The response is one JSON line. A successful START includes `task_id`, `pid`,
`pgid`, `start_token`, `state`, and `created`. A duplicate idempotency key
returns the same TaskInstance with `created=false`.

## Persistence and Recovery

SQLite stores the idempotency key, task ID, command, log path, PID, process
group ID, process start token, state, and timestamps. The start token prevents
PID reuse from being mistaken for the original task.

Startup recovery validates each persisted running task against its live process
identity. A stale socket is removed before rebinding. Daemon SIGKILL must not
terminate task processes.

## Test Sequence

1. Start the daemon and a three-level process tree.
2. Repeat START with the same key and verify the same PID is returned.
3. Query STATUS and persist the response.
4. SIGKILL the daemon and verify the task process group remains alive.
5. Restart the daemon using the same socket and database.
6. Verify STATUS recovers the same task ID, PID, process group, and start token.
7. STOP the task, verify the process group is gone, and query SQLite history.
