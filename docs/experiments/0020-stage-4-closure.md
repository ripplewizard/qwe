# Experiment 0020: Stage 4 Closure

Status: Passed on macOS ARM64 and Fedora Linux x86_64
Date: 2026-11-06
Environment: ulysses macOS 26.5.1 arm64; istoria Fedora Linux 35 x86_64

## Scope

This experiment closes the Stage 4 acceptance matrix and verifies the final
second-UID security gate.

The same `just stage4-check` command now includes:

- Workspace format, lint, unit tests, protocol schema, and stable test IDs.
- Service-manifest validation and real launchd/systemd lifecycle probes.
- Credential delivery, redaction, resource controls, dynamic policy updates,
  and forced teardown.
- Binary replacement, managed restart, worker adoption, and graceful stop.
- Local signed Keychain ACL, locked-keychain failure, and unlock recovery.
- Durable idempotency recovery.
- A real second-UID socket connection check.

## Second-UID Probe

The probe starts qwed in an isolated owner-only runtime directory, runs the
client as the unprivileged `qwe-probe` account, and verifies:

1. The OS denies connection through the private directory and socket.
2. A temporary ACL permits the second UID to connect.
3. qwed rejects that connected peer through its application-level UID check.

Observed results:

```json
{"case":"SEC-004","platform":"macOS","peer_uid":503,"os_socket_permissions":"pass","application_peer_uid_check":"pass"}
```

```json
{"case":"SEC-004","platform":"Fedora","peer_uid":977,"os_socket_permissions":"pass","application_peer_uid_check":"pass"}
```

Both hosts use a narrowly scoped sudoers rule for
`/tmp/qwe-cross-uid-client.py`; neither grants `NOPASSWD: ALL`.

## Gate Result

`just stage4-check` passes on both platforms with real qwed, worker, PTY,
SQLite, service-manager, Keychain, and second-UID processes. Linux reports hard
cgroup v2 memory, swap, task-count, and CPU-bandwidth controls. macOS reports
`monitored` libproc enforcement with bounded polling overshoot.

## Boundary

The second-UID probe uses an isolated runtime layout with the same owner-only
directory and socket semantics as the service assets. It is not packaged
installation acceptance. Packaged installation, upgrade and schema migration,
public Developer ID signing, notarization, release-mode soak measurements, and
full manual release acceptance remain Stage 5 gates.
