# Task Restart Policy POC Contract

Status: implemented and passing for Rust, Go, and C++.

## Operations

```text
RESTART_START <request-id> <task-key> <log-prefix> <policy>
              <max-attempts> <program> [args...]
RESTART_STATUS <request-id> <task-key>
```

Client commands:

```text
qwe-poc task-start-restart <socket> <key> <log-prefix> <policy>
        <max-attempts> <program> [args...]
qwe-poc task-restart-status <socket> <key>
```

Supported policies:

- `never`: execute once.
- `on-failure`: execute again only when the previous exit code is non-zero.
- `always`: execute again after every completion.

`max-attempts` must be between 1 and 20.

## TaskInstance History

Every execution attempt is stored independently with:

- task key
- attempt number
- PID and process group ID
- process start token
- state
- exit code
- start and end timestamps
- log path

The RESTART_STATUS response includes `task_key`, `policy`, `state`,
`attempts`, and an ordered `instances` array. The overall state is `running`
while attempts remain active and `completed` after the sequence ends.

RESTART_START is idempotent by task key. Repeating it returns the existing
sequence with `created=false`.

## Exit Semantics

Normal process exit codes are stored directly. Signal termination is recorded
as `128 + signal number`. Restart decisions use the recorded exit code.

Daemon restart recovery of an in-progress restart sequence is outside this POC
slice.
