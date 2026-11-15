# Experiment 0044: Stage 9.2 Agent Security

Date: 2026-11-14
Status: Pass for Stage 9.2

## Scope

This experiment validates Principal-bound capability enforcement and
revocation under [ADR 0032](../adr/0032-agent-execution-model.md). It extends
the Stage 9.1 real-process `exec` probe.

## Implemented Security Path

- Random capability tokens are stored in SQLite only as SHA-256 hashes.
- Capabilities bind an operation, global/Pane/command scope, expiry, Principal,
  and complete ancestor epoch snapshot.
- The CLI reads bootstrap or agent capabilities from mode-0600 files. Raw tokens
  are not command-line arguments or child environment values.
- The creating `agent_exec` capability can control only commands it created.
  Command-scoped read and terminate capabilities may be issued separately.
- Every agent request reloads the Principal ancestor chain. A revoked ancestor
  or changed epoch invalidates all descendant capabilities immediately.
- Administrative revocation preserves running work; freeze sends `SIGSTOP`;
  terminate kills the command group.
- Freeze timeout defaults to 60 seconds, is capped at five minutes, and is
  escalated by a qwed invigilator. Both the freeze and termination are visible
  command events.
- Administrative revocation and security holds require high-trust Principals.

## Verification

`just check-qwe` passes formatting, clippy with warnings denied, all workspace
tests, and the canonical protocol schema.

`just stage9-agent-exec` reports:

```json
{"case":"AGENT-001/AGENT-002/AGENT-003/AGENT-004/AGENT-005/AGENT-006/AGENT-007/AGENT-008","exec":"pass","split_streams":"pass","wait_timeout":"pass","signal":"pass","deadline":"pass","bounded_replay":"pass","pane_resource_scope":"pass","cli":"pass","qwed_restart":"pass","lost_state":"pass","capability_scope":"pass","ancestor_revocation":"pass","administrative_revocation":"pass","freeze_escalation":"pass","low_trust_denial":"pass","capability_redaction":"pass"}
```

The probe covers missing, expired, cross-Pane, creator-command, and
command-scoped capabilities; ancestor revocation; preserved administrative
revocation; default freeze; finite hold/grace escalation; low-trust
administrative denial; CLI bootstrap/issue/write mode; qwed restart; and
absence of raw token values in the SQLite database or WAL. An insecure
mode-0644 capability file is rejected before any request is sent.

## Remaining Work

Stage 9.3 subsequently added per-command descriptor credentials and mandatory
redaction in [Experiment 0045](0045-stage9-command-credentials.md). Stage 9.4
subsequently added read-only observer/push and PTY merged output. Stage 9.5
subsequently added foreground process-group detection and real `idle`.

The local root bootstrap file remains readable by any process with the same UID.
Capabilities stop ambient authority between Principals but do not replace OS
multi-user isolation.
