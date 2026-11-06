# Experiment 0021: Stage 5 Release Foundation

Status: Foundation passing on macOS ARM64; Linux and public distribution pending
Date: 2026-11-06
Base commit: `a8dea54` with Stage 5 worktree changes
Environment: macOS 26.5.1 arm64, release profile

## Scope

This experiment starts Stage 5 with a repeatable native release package,
isolated installation, offline package verification, upgrade-state acceptance,
and release-mode Attach performance measurement.

The release package contains `qwe`, `qwed`, `qwe-worker`,
`qwe-session-exec`, launchd and systemd assets, an installer, a
`qwe-release/v1` manifest, and SHA-256 checksums. All four executables expose
their package version.

## Installation and Upgrade

The isolated-home probe passed:

```json
{"case":"PACK-003","package":"pass","archive_extraction":"pass","temporary_home_install":"pass","service_assets":"pass","database_preserved_during_reinstall":"pass","worker_adopted_after_restart":"pass","schema_v9_to_v10":"pass"}
```

The probe checked archive-safe extraction, package checksums, installed
executable permissions, binary and protocol versions, native service-template
rendering, and absence of the development checkout path. It created a live
Session, killed qwed, reinstalled the package into the same prefix, restarted
qwed, and verified the same worker and task were adopted while SQLite remained
byte-identical during install. It then replaced the Pane table with an
equivalent v9 layout and verified startup migration to v10.

## Release Benchmark

`just release-benchmark 15` used eight independent Sessions emitting PTY
output. Each measurement opened the real Attach channel, received
`ATTACH_ACCEPTED` and `SNAPSHOT`, detached, and repeated across eight rounds
plus the sustained soak.

Observed:

```json
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":568,"soak_seconds":15.0,"p50_ms":18.412,"p95_ms":23.893,"p99_ms":24.7,"max_ms":26.002,"max_rss_bytes":172834816,"max_p95_ms":1000,"max_p99_ms":2000,"max_rss_limit_bytes":536870912}
```

These thresholds are release-gate regression bounds for the tested machine,
not cross-platform performance claims.

## Gate Result

`just release-gate` passes on macOS ARM64 with the package, archive extraction,
isolated installation, reinstall, worker-adoption, schema-migration, and
release-mode benchmark probes above. The source was dirty because this
experiment records the worktree before its Stage 5 commit.

## Boundary

macOS package installation, reinstall, worker adoption, schema migration, and
the release benchmark pass. Fedora package acceptance and benchmarking have
not run at this commit. Full manual release acceptance is pending.

No public Apple Developer identity is available. Developer ID signing,
notarization, and Gatekeeper public-trust acceptance remain unpassed and are
not claimed by this experiment.
