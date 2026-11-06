# Experiment 0025: Stage 5 Linux V0 Closure

Status: Complete
Date: 2026-11-06
Code commit: `8d294df`
Reviewer and submitter: `hjian`

## Release Scope

V0 is released Linux-first on Fedora Linux x86_64 under ADR 0020. macOS ARM64
remains an experimental local self-signed target under ADR 0019; public macOS
distribution is deferred to a separate post-V0 milestone.

## Final Evidence

| Gate | Result |
|---|---|
| `just release-gate` on Fedora x86_64 | PASS |
| `just stage4-check` on Fedora x86_64 | PASS |
| PACK-003 package install/reinstall/migration | PASS |
| PACK-005 installed-systemd crash restart/adoption | PASS |
| PERF-001 release Attach/soak | PASS |
| Neovim/htop TUI smoke | PASS |
| Ctrl-Z/jobs/bg/fg/Ctrl-C and reattach smoke | PASS |
| Independent manual T03/T04/T08/T11 review | PASS |
| Historical evidence audit | PASS without rewriting failed results |

Final release performance:

```json
{"case":"PERF-001","build_profile":"release","sessions":8,"attach_requests":552,"soak_seconds":15.0,"p50_ms":8.384,"p95_ms":26.14,"p99_ms":26.504,"max_ms":26.809,"max_rss_bytes":153911296}
```

Final Linux archive:

```text
dist/qwe-0.1.0-linux-x86_64.tar.gz
SHA-256: 7835ddc5a23e8c85be88b01051b6415493d34b6b09b5231084ea3e704cc681bf
```

Manual evidence remains local under
`artifacts/stage5-linux-manual-20261106/`, including terminal captures,
session status, qwed logs, and checksums.

## Capability Boundary

- Fedora Linux x86_64 is the first supported V0 target.
- macOS ARM64 is local/experimental with `public_trust: false`.
- macOS x86_64 and Linux ARM64 are not qualified.
- Public macOS binaries require a later Developer ID/notarization milestone.
- QWE Mesh, sandboxing, plugin systems, and remote scheduling remain outside
  V0 scope.

No known P0 design contradiction remains for the Linux-first V0 release.
