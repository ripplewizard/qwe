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
| STAGE6-001 | Core CI runs formatting, Clippy, tests, protocol validation, and release builds | Workflow added; remote execution pending |
| STAGE6-002 | A dedicated Fedora x86_64 self-hosted runner executes `just release-gate` | Workflow added; runner registration and first run pending |
| STAGE6-003 | A clean-source Linux x86_64 package and checksum are published as a versioned release asset | Publication pending |
| STAGE6-004 | Pilot reports are triaged, and no open Linux P0/P1 defect blocks `v0.1.x` | Pilot not started |

The existing local `dist/` archives are generated evidence, not release
assets. They are ignored and may not match a tagged clean source revision.
Stage 6 must rebuild the published artifact from the release commit and verify
its manifest, checksum, installation, service recovery, and performance gates.

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
