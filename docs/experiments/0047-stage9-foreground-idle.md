# Experiment 0047: Stage 9.5 Foreground Idle

Date: 2026-11-14
Status: Pass for Stage 9.5 and the accepted Agent Execution first delivery

## Scope

This experiment validates interactive-shell foreground-group detection under
[ADR 0032](../adr/0032-agent-execution-model.md).

## Implementation

PTY commands retain the master descriptor while the command is active. For a
shell executable in interactive mode, the worker calls `tcgetpgrp(master_fd)`
and compares the foreground group with the shell command's own process group:

- shell itself foreground -> state `idle`, commit `idle_observed`;
- foreground child group -> state `running`;
- terminal process state -> `done`, `failed`, or `lost`, checked first.

Non-PTY commands and shell command-string modes such as `sh -c` never claim
idle. The PTY master is cleared after terminal readers drain, preventing a
descriptor leak for retained command metadata.

## Verification

`just check-qwe` passes formatting, clippy with warnings denied, workspace
tests, and protocol schema validation.

`just stage9-agent-exec` reports AGENT-001 through AGENT-012 passing, including:

- interactive `/bin/sh` PTY command returning `idle`;
- `AGENT_COMMAND_STATUS` reporting `idle`;
- `idle_observed` committed to retained events;
- termination from idle using the same process-group path;
- one-shot PTY processes still waiting for `done` rather than falsely reporting
  idle;
- all prior lifecycle, replay, security, credential, observer, and PTY checks.

## Delivery Status

Stages 9.1-9.5 complete the accepted first Agent Execution delivery. Deferred
work remains explicitly separated: `shell_exec`, external sandbox integration,
cross-Pane orchestration, persistent semantic history, environment mutation,
host management, and multi-writer commands.
