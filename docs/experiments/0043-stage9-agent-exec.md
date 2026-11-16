# Experiment 0043: Stage 9.1 Agent Exec

Date: 2026-11-14
Status: Pass for Stage 9.1

## Scope

This experiment validates the first vertical slice of
[ADR 0032](../adr/0032-agent-execution-model.md): explicit non-PTY `exec`,
command handles, timeout-aware waiting, split streams, bounded replay, recovery,
and explicit worker-loss state.

At the Stage 9.1 boundary this experiment does not claim the full accepted
delivery. Later experiments add capability/revocation, credentials/redaction,
observer push, PTY output, and foreground `idle`.

## Topology Correction

The initial implementation attempted to move a spawned command from the worker
cgroup into the existing Pane scope. Real-systemd validation rejected the move
with `EPERM`: cgroup v2 migration requires write access through the source and
destination common ancestor.

The corrected production topology starts the Pane worker itself inside the
transient Pane scope. The worker then uses delegated `control` and `workload`
child cgroups: worker infrastructure stays in `control`, while the PTY task
and agent commands share `workload`. This keeps user limits off worker threads
without escaping the Pane scope. The probe verifies the command and PTY task
share one workload cgroup and that the worker control cgroup has the same
parent scope.

## Verification

`just check-qwe` passes formatting, clippy with warnings denied, the workspace
tests, and protocol schema validation.

`just stage9-agent-exec` reports:

```json
{"case":"AGENT-001/AGENT-002/AGENT-003/AGENT-004/AGENT-005/AGENT-006","exec":"pass","split_streams":"pass","wait_timeout":"pass","signal":"pass","deadline":"pass","bounded_replay":"pass","pane_resource_scope":"pass","cli":"pass","qwed_restart":"pass","lost_state":"pass"}
```

The probe uses real qwed, worker, PTY, Unix sockets, SQLite, process groups,
systemd scopes, and commands. It covers:

- exact exit code 7 in `done`;
- separate stdout and stderr byte streams;
- start failure and signaled `failed` states;
- absolute deadline kill;
- a short wait producing `unknown`;
- more than 1 MiB of output producing an explicit replay gap;
- worker-control and PTY/command-workload placement under one Pane scope;
- CLI execution, wait, and event replay;
- qwed kill/restart while the worker and command survive; and
- worker SIGKILL producing `lost`, not `done`.

Regression gates also pass:

- `just stage8-layout`: LAYOUT-001 through LAYOUT-017.
- `just stage7-resource-profile`: RG-001/RG-002/RG-003/RG-006.
- `just stage7-multi-pane`: SHARED-002/SHARED-004/SHARED-005/SHARED-006.

## Next Slice

Stage 9.2 subsequently added command capabilities and Principal-bound
revocation in [Experiment 0044](0044-stage9-agent-security.md), and Stage 9.3
subsequently added descriptor credentials and mandatory redaction in
[Experiment 0045](0045-stage9-command-credentials.md). Observer/push and PTY
output follow without changing command identity or the Pane execution path.
