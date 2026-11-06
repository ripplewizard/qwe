# Experiment 0022: Stage 5 Linux Release Candidate

Status: Automated Linux release gate passing; independent manual acceptance pending
Date: 2026-11-06
Commit: `97328cb` with only the pre-existing untracked `.vscode/`
Environment: Fedora Linux 35 x86_64, systemd user manager running

## Scope

This experiment closes the installed-service gap in the Stage 5 Linux release
gate while macOS work is paused.

The package probe now:

1. Installs the checksummed release into an isolated home prefix.
2. Copies the installed systemd unit under a unique validation name.
3. Uses a validation drop-in to redirect runtime and SQLite paths without
   weakening the production manifest.
4. Starts qwed through the real systemd user manager.
5. Creates a live Session and task.
6. Reinstalls the package over the running installation.
7. SIGKILLs qwed and waits for systemd to restart it.
8. Verifies the replacement qwed adopts the same worker and task.
9. Stops the service and verifies worker/task cleanup plus durable
   `closed`/`stopped` state.

## Result

```json
{"case":"PACK-005","installed_systemd_unit":"pass","binary_reinstall_preserved_store":"pass","systemd_crash_restart":"pass","worker_and_task_adopted":"pass","graceful_stop_cleanup":"pass"}
```

The full `just release-gate` also passed workspace checks, protocol schema,
persisted state probes, package extraction/install/migration, release-mode
performance, and TUI smoke.

Linux release performance:

```json
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":568,"soak_seconds":15.0,"p50_ms":8.461,"p95_ms":26.258,"p99_ms":26.512,"max_ms":26.655,"max_rss_bytes":153812992,"max_p95_ms":1000,"max_p99_ms":2000,"max_rss_limit_bytes":536870912}
```

Neovim and htop detach/reattach smoke passed:

```text
TUI attach smoke PASS (nvim, htop)
```

The cumulative Linux security gate also passed:

```text
just stage4-check
```

This revalidated managed service lifecycle and binary replacement, credential
descriptor redaction, hard cgroup resource controls, malformed/oversized input,
forced teardown, idempotency, and a real second-UID application rejection
(peer UID 977).

The generated Linux archive SHA-256 was
`8ddff02a02b8e201dc9f038facb0a3f8636ff8daa537849a54f72a872351b28b`.
The source was marked dirty only because `.vscode/` is untracked; tracked and
staged diffs were clean.

## Boundary

Automated Linux release validation is complete for this candidate. Independent
manual release acceptance and final POC/manual audit disposition remain open.
Public macOS distribution and all further macOS development/testing are
deferred until Linux has an accepted release and a Developer ID account is
available.
