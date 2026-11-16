# Experiment 0045: Stage 9.3 Command Credentials

Date: 2026-11-14
Status: Pass for Stage 9.3

## Scope

This experiment validates per-command credential delivery and redaction under
[ADR 0032](../adr/0032-agent-execution-model.md).

## Descriptor Path

qwed reads a named credential from the existing private provider into
`Zeroizing` memory and writes it to a pipe. A one-shot mode-0600 Unix listener
accepts the owning Pane worker and sends the read descriptor with `SCM_RIGHTS`.
Only the broker path and `credential_exact` policy appear in worker control JSON.

The worker receives the descriptor, validates a nonempty value of at most
16 KiB, creates a fresh child pipe, and maps it to FD3 through
`qwe-session-exec`. The child sees only `QWE_CREDENTIAL_FD=3`.

The same zeroizing secret value drives independent streaming redactors for
stdout and stderr. Complete matches and partial trailing prefixes become
`[REDACTED]` before events are retained; the first match emits
`redaction_applied`.

## Verification

`just check-qwe` passes formatting, clippy with warnings denied, workspace
tests, and protocol schema validation.

`just stage9-agent-exec` reports:

```json
{"case":"AGENT-001/AGENT-002/AGENT-003/AGENT-004/AGENT-005/AGENT-006/AGENT-007/AGENT-008/AGENT-009","exec":"pass","split_streams":"pass","wait_timeout":"pass","signal":"pass","deadline":"pass","bounded_replay":"pass","pane_resource_scope":"pass","cli":"pass","qwed_restart":"pass","lost_state":"pass","capability_scope":"pass","ancestor_revocation":"pass","administrative_revocation":"pass","freeze_escalation":"pass","low_trust_denial":"pass","capability_redaction":"pass","command_credential_descriptor":"pass","command_credential_redaction":"pass","credential_failure_closed":"pass"}
```

The real-process probe verifies:

- the child reads the exact credential from FD3;
- the secret is absent from argv, environment values, and `QWE_*` values;
- a secret split across stdout writes is redacted as one value;
- stderr is redacted independently;
- `redaction_applied` is present in the retained event stream;
- the secret is absent from SQLite and WAL bytes and qwed/worker logs;
- missing and empty credentials fail without false success.

Unit coverage additionally checks redaction across fixed output chunk
boundaries and incomplete trailing secret prefixes.

## Remaining Work

Stage 9.4 subsequently added read-only observer/push and PTY merged output in
[Experiment 0046](0046-stage9-observer-pty.md). Stage 9.5 subsequently added
foreground process-group detection and real `idle`.
