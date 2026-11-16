# Experiment 0050: Agent Command Signal API

Date: 2026-11-15
Status: Pass for AGENT-013

## Result

`AGENT_COMMAND_SIGNAL` adds targeted signal delivery without turning command
termination into an arbitrary signal interface.

Accepted signals:

- `SIGINT`
- `SIGTERM`
- `SIGHUP`
- `SIGQUIT`

`SIGKILL`, numeric signals, and other names return `invalid_request`. Forced
termination remains exclusively `AGENT_COMMAND_TERMINATE` and Pane cgroup
cleanup.

## Target Semantics

- PTY commands use the current foreground process group when available.
- Non-PTY commands use the command process group.
- A successful delivery appends a `signal_delivered` command event.
- The creating `agent_exec` capability may signal its commands.
- A command-scoped `agent_command_terminate` capability may also signal.
- Read-only observer capabilities remain unable to signal.

Signals are delivered through the same process-group selection used by
foreground idle and termination. They do not mark a command terminal
preemptively; the normal wait result records the actual exit code or signal.

## Verification

- non-PTY SIGTERM reaches a sleeping process and returns `failed/SIGTERM`;
- sandboxed non-PTY SIGTERM reaches the Bubblewrap command and returns
  `failed/SIGTERM`;
- sandboxed interactive PTY SIGINT reaches the foreground shell and leaves the
  command alive;
- `signal_delivered` events record the requested signal;
- observer signal attempts return `capability_scope_denied`;
- SIGKILL is rejected; and
- CLI `agent signal` uses the same idempotent operation.

Commands:

```sh
just stage9-agent-exec
just stage10-sandbox
```

## Not Included

This experiment does not add arbitrary PTY input or resize. Those remain
separate product capabilities for interactive Agent terminals.
