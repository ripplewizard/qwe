# ADR 0020: Linux-First V0 Release

Status: Accepted
Date: 2026-11-06

## Context

Linux automated release validation is complete on Fedora x86_64. macOS public
binary distribution remains blocked on an Apple Developer Program account and
has been paused. Keeping public macOS notarization as a Stage 5 release blocker
would delay Linux adoption without validating any Linux runtime capability.

## Decision

- V0 release acceptance prioritizes Fedora Linux x86_64.
- Linux x86_64 becomes the first supported V0 release target after independent
  manual acceptance and the final evidence audit pass.
- macOS ARM64 remains an experimental, machine-local self-signed target under
  ADR 0019. It is not a public binary distribution claim and does not block the
  Linux-first V0 release.
- Public macOS distribution moves to a separate post-V0 release milestone.
  It must still satisfy Developer ID signing, notarization, Gatekeeper, manual
  acceptance, and structured evidence before support is claimed.
- V0 scope is frozen to the documented single-node Session/Pane/Task model.
  Linux-first release does not authorize Mesh, sandboxing, plugin, remote
  scheduling, or other research extensions.

## Exit Criteria

The Linux-first V0 release is accepted when:

1. `just release-gate` passes on the release commit.
2. An independent reviewer completes the Linux manual release checklist.
3. The historical POC/manual evidence audit is complete without rewriting
   failed results.
4. The supported-platform matrix and V0 limitations are published.
5. The release branch is merged and the candidate revision is tagged.

## Consequences

- Linux engineering can proceed without waiting for Apple account approval.
- macOS implementation and public-distribution work are explicitly deferred,
  not silently removed.
- A future macOS release can restore ADR 0019's public path without changing
  the packaged runtime.
- Claims remain split between Linux supported, macOS local/experimental, and
  macOS public unavailable.
