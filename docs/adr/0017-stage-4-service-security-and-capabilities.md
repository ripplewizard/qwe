# ADR 0017: Stage 4 Service, Secret, Resource, and Teardown Contracts

Status: Accepted
Date: 2026-11-05
Implementation state: Core runtime behavior, capability reporting, service
assets, managed crash-restart/stop lifecycle, local signed Keychain ACL,
and binary-replacement restart are implemented. Installed cross-UID acceptance
and public Developer ID distribution remain open.

The initial per-process Pane resource contract is superseded by
[ADR 0018](0018-pane-resource-enforcement.md); secret, service, and teardown
decisions in this ADR remain active.

## Context

ADR 0010 requires an API Key Manager, Pane resource policy, and forced
teardown. The Stage 0 macOS probe established that Keychain retrieval is
feasible, while macOS lacks a proven Pane-level memory or process-tree
enforcement primitive. `portable-pty` also closes descriptors above standard
input/output/error after fork, so a direct descriptor handoff from qwed through
the PTY spawn path is not portable.

Stage 4 needs stable behavior that does not silently substitute files or
environment variables for unsupported security and resource capabilities.

## Decision

### Secrets

- qwed is the credential broker. Credential sources are a private credential
  directory (including systemd's `CREDENTIALS_DIRECTORY`) or, on macOS, a
  Keychain generic-password item named `<service-prefix>.<credential-name>`.
- Filesystem credentials must be regular files, not symlinks, mode `0600` or
  stricter. The directory must be mode `0700` or stricter. Credentials are
  limited to 16 KiB.
- qwed reads a credential only after a `SESSION_CREATE` request names it. The
  bytes are held in zeroizing memory and sent through an owner-only, one-shot
  Unix socket in qwed's private runtime directory.
- The internal `qwe-session-exec` launcher maps that socket to descriptor 3,
  clears close-on-exec, and sets only `QWE_CREDENTIAL_FD=3` before exec. The
  secret itself never enters argv, the environment, workspace files, logs, or
  SQLite.
- Missing, insecure, oversized, or inaccessible credentials fail the creation
  with `credential_unavailable`. No fallback environment or workspace-file
  provider exists.
- `CONTROL_STATUS` may list credential names and the active backend. It never
  returns secret bytes.

The one-shot socket exists because `portable-pty` closes all inherited
descriptors above 2 after fork. It is a transport for the final inherited
descriptor, not durable secret storage.

### Pane resources

- Pane policy is persisted as optional `cpu_seconds`, `memory_bytes`, and
  `process_limit`.
- CPU uses a hard `RLIMIT_CPU` backstop on Linux and macOS. It limits each
  process, not aggregate Pane CPU.
- Memory uses `RLIMIT_AS` on Linux. macOS reports memory policy as unsupported.
- Process-count policy is unsupported on both platforms because `RLIMIT_NPROC`
  is UID-wide rather than Pane-tree scoped.
- A request for an unsupported policy fails with `unsupported_capability`.
  QWE never silently drops a requested limit.
- `CONTROL_STATUS.capabilities.resources` reports the exact supported model and
  its per-process limitation.

### Forced teardown

- `SESSION_KILL` requires the same Active/Held authority as normal Session
  teardown and sends `SIGKILL` to the Task process group.
- `TASK_KILL_BATCH` accepts 1-100 TaskIds, authorizes each Task's Session,
  continues independent teardown when one item fails, and returns per-item
  results.
- `SERVER_KILL` requires `confirm=true`. It force-kills every non-closed Session
  and worker, persists terminal history and authority release, sends its
  response, and exits qwed.
- Forced teardown retains Session, Pane, Task, and Lease tombstones. It does not
  resurrect or delete terminal history.

### Services

- Linux uses a systemd user unit with private runtime/state directories and
  explicit restart policy. `KillMode=process` preserves workers across a qwed
  crash so restart can adopt them; qwed handles graceful `SIGTERM` by tearing
  down all Sessions and workers before exiting.
- macOS uses a launchd user agent and wrapper that stores state under
  `~/Library/Application Support/QWE`.
- Service manifests are installation assets. Managed SIGKILL restart, worker
  adoption, and graceful stop cleanup are tested with real launchd/systemd
  jobs. Binary replacement remains a separate upgrade-acceptance check.

## Consequences

- A credential passed to a non-cooperating program remains readable through
  descriptor 3 until that process exits or closes it; QWE cannot revoke an
  inherited descriptor after exec.
- CPU limits terminate a process with `SIGXCPU`; they are a defensive backstop,
  not aggregate Pane accounting.
- Memory and process-tree limits are unavailable on macOS without a new
  isolated platform primitive.
- `kill-server` is a same-UID administrative operation and therefore does not
  require per-Session attachment authority. Session and batch teardown do.

## Acceptance Required

- Verify a real cross-UID client rejection in the installed socket layout.

Resource-enforcement acceptance now follows ADR 0018.
Public Developer ID trust and notarized release packaging are Stage 5 gates.
