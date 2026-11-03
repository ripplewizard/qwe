# Extension Boundary Review

Status: complete for the POC scope.

## Core Reservations

The stable POC objects are Session, TaskInstance, process group, and terminal
state. Extension data may reference these objects but must not take ownership
of their lifecycle.

- TaskInstance keeps a stable ID and OS process identity.
- Pane must not own TaskInstance.
- Session lease expiration must not terminate the Task process.
- Process Runtime must not depend on CLI or TUI state.
- Local paths and PIDs must not become global identity.

## Extension Registry

The following directions remain research or deferred scope:

- E01 QWE Mesh: node identity, remote target, authorization, and migration.
- E02 Resource Controller: monitoring first, enforcement later.
- E03 Credential Broker: scoped grants without exposing raw secrets.
- E04 Security Sandbox: backend-specific isolation policy.
- E05 Agent Runtime: agents modeled as Tasks.
- E06 Environment Backend: workspace and execution environment abstraction.
- E07 Terminal Backend: PTY and VT implementations behind replaceable APIs.

## Material Findings

- The POC object split is directionally compatible with later extension work.
- Pane and Window lifecycle are not implemented in the POC and are unproven.
- IPC has request IDs and stable errors, but production version negotiation is
  outside the POC.
- The raw-stream Attach path is unsuitable as the production terminal API.
- Terminal recovery boundary is not met by the POC and must be resolved by a
  durable terminal host or explicitly accepted terminal-loss semantics.

## Review Limits

This is a source and design-boundary review, not a production threat model or
plugin security review.
