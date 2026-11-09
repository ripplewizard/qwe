# Experiment 0026: Stage 6 Adoption Baseline

Status: In progress
Date: 2026-11-06
Entry commit: `7ca5ee4`

## Entry State

Stage 5 closed Linux-first V0 on Fedora Linux x86_64. The supported artifact
contract, isolated installation, reinstall, SQLite migration, installed-systemd
recovery, release-mode Attach/soak, TUI, and job-control checks pass.

Stage 6 starts from the `dev` integration branch with four adoption gates:

| ID | Gate | Entry state |
|---|---|---|
| STAGE6-001 | Core CI runs formatting, Clippy, tests, protocol validation, and release builds | Pass on GitHub-hosted Linux and macOS at `66e05a8` |
| STAGE6-002 | A dedicated Fedora x86_64 runner executes `just release-gate` | Workflow implemented; runner registration and first workflow run pending |
| STAGE6-003 | A clean-source Linux x86_64 package and checksum are published as a versioned release asset | Publication pending |
| STAGE6-004 | Pilot reports are triaged, and no open Linux P0/P1 defect blocks `v0.1.x` | Pilot not started |

The existing local `dist/` archives are generated evidence, not release
assets. They are ignored and may not match a tagged clean source revision.
Stage 6 must rebuild the published artifact from the release commit and verify
its manifest, checksum, installation, service recovery, and performance gates.

## Initial Evidence

GitHub Actions run
[42179400285](https://github.com/ripplewizard/qwe/actions/runs/42179400285)
passed both workspace jobs at commit `66e05a8`:

- `Workspace (ubuntu-latest)`: success.
- `Workspace (macos-latest)`: success.
- Both jobs passed formatting, Clippy, workspace tests, canonical protocol
  validation, and release builds.

The ordinary Fedora checkout contained an untracked `.vscode/settings.json`.
The first release-gate run passed but correctly produced
`source_dirty: true`, so that archive was rejected as a release candidate. The
complete gate was rerun in a detached clean worktree at `66e05a8` without
removing the existing test-host file. The clean run passed:

| Gate | Result |
|---|---|
| `just check-qwe` | PASS |
| Persisted state and Stage 3 authority/idempotency/reconciliation | PASS |
| PACK-001 package manifest and SHA-256 | PASS |
| PACK-003 install, reinstall, worker adoption, v9-to-v10 migration | PASS |
| PACK-005 installed systemd crash restart and cleanup | PASS |
| PERF-001 release Attach/soak | PASS |
| Neovim/htop and job-control TUI smoke | PASS |
| Manifest `source_dirty` | `false` |

Clean Linux archive:

```text
dist/qwe-0.1.0-linux-x86_64.tar.gz
SHA-256: 968a2e5a5bb4e1d1032e0bc6e757144ee9e532eb3a377b4e766ac83822faa9b0
source_commit: 66e05a893ffc
source_dirty: false
```

Release performance:

```json
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":560,"soak_seconds":15.0,"p50_ms":8.953,"p95_ms":25.632,"p99_ms":25.998,"max_ms":26.435,"max_rss_bytes":153763840}
```

These results establish the Stage 6 CI and release-gate path. STAGE6-002 is
still open because the clean Fedora gate ran through an SSH control session,
not the dedicated repository runner. STAGE6-003 and STAGE6-004 are also open:
no release asset has been published and no user pilot has run.

## Initial Scope

- Keep Fedora Linux x86_64 as the only supported V0 target.
- Treat Ubuntu CI as portability/compile coverage, not a new support claim.
- Collect and triage real Linux pilot feedback through the repository issue
  form.
- Preserve SQLite and user terminal history across `v0.1.x` upgrades.
- Keep public macOS distribution and extension features outside Stage 6.

## Closure Evidence

Closure requires linked CI runs, a Fedora release-gate run, the published
artifact URL and SHA-256, clean-source manifest, install smoke, and the pilot
triage summary. No gate is complete until that evidence exists.
