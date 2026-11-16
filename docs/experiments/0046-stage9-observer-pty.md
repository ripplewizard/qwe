# Experiment 0046: Stage 9.4 Observer and PTY

Date: 2026-11-14
Status: Pass for Stage 9.4

## Scope

This experiment validates read-only command observation and PTY-merged output
under [ADR 0032](../adr/0032-agent-execution-model.md).

## Observer

`AGENT_COMMAND_OBSERVE` requires a command-scoped `agent_command_read`
capability or the creating `agent_exec` capability. qwed rechecks the live
Principal epoch chain before each poll and pushes redacted event batches over the
existing owner-only control socket. A 32-frame bounded `SyncSender` isolates
slow subscribers; writer timeouts and queue exhaustion disconnect rather than
forcing unbounded buffering.

The observer receives committed metadata and output only. It cannot input,
resize, signal, terminate, or otherwise mutate the command. A terminate request
using only the read capability fails with `capability_scope_denied`.

## PTY

`AGENT_EXEC` with `pty:true` allocates a fresh PTY and exposes one merged `pty`
event stream. The launcher blocks on the existing topology socket until the
worker has moved the process into the Pane workload cgroup. PTY credentials use
a one-shot worker-local broker and map to child FD3. Exact-value redaction is
applied before the merged output enters retention.

The probe verifies terminal control bytes, CR/LF translation, split-chunk
credential redaction, merged cursor semantics, command completion, and the
absence of a second command identity or state path.

## Verification

`just check-qwe` passes formatting, clippy with warnings denied, all workspace
tests, and the protocol schema.

`just stage9-agent-exec` reports AGENT-001 through AGENT-011 passing, including:

- multi-frame observer push and CLI consumption;
- observer read-only capability denial;
- PTY merged output with terminal control sequences;
- PTY descriptor credentials and split-chunk redaction;
- all Stage 9.1-9.3 lifecycle, replay, recovery, and security checks.

## Remaining Work

Stage 9.5 subsequently added foreground process-group detection and real
interactive-shell `idle` in
[Experiment 0047](0047-stage9-foreground-idle.md).
