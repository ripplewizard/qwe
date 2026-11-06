# Experiment 0021: Stage 5 Release Foundation

Status: Foundation passing on macOS ARM64 and Fedora Linux x86_64; public
distribution and manual acceptance pending
Date: 2026-11-06
Commits: macOS `a0dfeb0`; Fedora `c59048e` with only an untracked `.vscode/`
Environment: macOS 26.5.1 arm64; Fedora Linux 35 x86_64; release profile

## Scope

This experiment starts Stage 5 with a repeatable native release package,
isolated installation, offline package verification, upgrade-state acceptance,
and release-mode Attach performance measurement.

The release package contains `qwe`, `qwed`, `qwe-worker`,
`qwe-session-exec`, launchd and systemd assets, an installer, a
`qwe-release/v1` manifest, and SHA-256 checksums. All four executables expose
their package version.

macOS artifacts use the temporary local self-signed build admitted by
[ADR 0019](../adr/0019-temporary-macos-local-signing.md). They carry the
`-local` suffix and record `public_trust: false`; Fedora artifacts remain
unsigned local packages because they do not use Apple distribution trust.

## Temporary Local Signing

The isolated local-signing lifecycle probe passed:

```json
{"case":"MAC-001","identity_lifecycle":"pass","codesign_verify":"pass","public_trust":false}
```

It created a temporary self-signed code-signing identity in a dedicated
Keychain, signed a release executable, verified the signature, removed the
identity and trust setting, and confirmed the user's Keychain search list was
restored. The actual release identity remains configured only on this macOS
host and is excluded from Git.

## Installation and Upgrade

The isolated-home probe passed on both platforms:

```json
{"case":"PACK-003","package":"pass","archive_extraction":"pass","distribution_profile":"local-self-signed","public_trust":false,"temporary_home_install":"pass","service_assets":"pass","database_preserved_during_reinstall":"pass","worker_adopted_after_restart":"pass","schema_v9_to_v10":"pass"}
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
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":600,"soak_seconds":15.0,"p50_ms":17.757,"p95_ms":20.154,"p99_ms":20.453,"max_ms":22.733,"max_rss_bytes":171982848,"max_p95_ms":1000,"max_p99_ms":2000,"max_rss_limit_bytes":536870912}
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":536,"soak_seconds":15.0,"p50_ms":9.268,"p95_ms":26.108,"p99_ms":26.403,"max_ms":26.627,"max_rss_bytes":153726976,"max_p95_ms":1000,"max_p99_ms":2000,"max_rss_limit_bytes":536870912}
```

These thresholds are release-gate regression bounds for the tested machine,
not cross-platform performance claims.

The first line is macOS ARM64; the second is Fedora Linux x86_64. Fedora
produced archive SHA-256
`a28472903a23292c55a2d87ee7a14c3808f68eef056334011c09b628c6c656e6`.
Its tracked and staged diffs were clean; the package marked the source dirty
because the pre-existing `.vscode/` directory is untracked.

## Gate Result

`just release-gate` passes on both supported targets with the package, archive
extraction, isolated installation, reinstall, worker-adoption,
schema-migration, and release-mode benchmark probes above. The macOS artifact
is self-signed locally and explicitly non-public. Passing that archive to the
public gate exits before notarization with
`archive distribution profile is local-self-signed`.

## Boundary

macOS and Fedora package installation, reinstall, worker adoption, schema
migration, and release benchmarks pass. Full manual release acceptance remains
pending.

No public Apple Developer identity is available. Developer ID signing,
notarization, and Gatekeeper public-trust acceptance remain unpassed and are
not claimed by this experiment.
