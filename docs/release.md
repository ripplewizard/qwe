# V0 Release

Status: Stage 5 in progress
Date: 2026-11-06

## Supported Platform Matrix

V0 qualification currently targets these native development platforms:

| Target | Status | Service manager | Resource enforcement | Credential provider |
|---|---|---|---|---|
| macOS ARM64 | Qualified through Stage 4; Stage 5 in progress | launchd user agent | libproc monitoring, explicitly monitored | macOS Keychain |
| Fedora Linux x86_64 | Qualified through Stage 4; Stage 5 in progress | systemd user service | cgroup v2 hard limits | private credential directory and one-shot broker |
| macOS x86_64 | Not qualified for V0 | unsupported claim | unsupported claim | unsupported claim |
| Linux ARM64 | Not qualified for V0 | unsupported claim | unsupported claim | unsupported claim |
| Other operating systems | Unsupported in V0 | unsupported claim | unsupported claim | unsupported claim |

The matrix describes tested targets, not cross-compilation promises. A target
moves to supported only after the structured release gates pass on that target.

## Local Release Gate

`just release-gate` performs:

1. Workspace formatting, lint, unit, and protocol-schema checks.
2. The persisted state matrix and real idempotency, authority, and
   reconciliation probes.
3. An optimized workspace build.
4. A checksummed native package containing `qwe`, `qwed`, `qwe-worker`,
   `qwe-session-exec`, native service assets, and a release manifest.
5. Safe archive extraction and installation into an isolated home prefix,
   followed by package-version and service-template verification.
6. In-place reinstall with SQLite preservation, qwed crash/restart worker
   adoption, and SQLite v9-to-v10 migration.
7. A release-mode benchmark with eight independent Sessions, rotating
   exclusive Attach, bounded memory sampling, and a sustained soak period.

The local gate generates runtime artifacts under ignored `dist/`. Structured
results may be summarized in a checked-in experiment; raw terminal captures,
SQLite state, logs, and environment dumps remain local.

## Package Contract

The release directory and archive use:

```text
qwe-VERSION-TARGET/
  README.md
  INSTALL.md
  SHA256SUMS
  release.json
  install.py
  bin/
    qwe
    qwed
    qwe-session-exec
    qwe-worker
  share/qwe/service/
    launchd/
    systemd/
```

`release.json` uses schema `qwe-release/v1` and records product and IPC
protocol versions, normalized target, source revision and dirty status,
individual executable hashes, service assets, and the signing identity when
one is selected. `SHA256SUMS` covers all payload files except itself and the
manifest.

The installer validates checksums before replacing executables. It does not
modify SQLite or start a service, so reinstall and binary replacement preserve
Session state for supervised restart.

## Public macOS Distribution

An ad-hoc or local certificate is not a public release identity. Public macOS
distribution requires:

1. `QWE_DEVELOPER_ID` naming the selected Developer ID Application identity.
2. `QWE_NOTARY_PROFILE` naming a `notarytool` keychain profile.
3. A release package built with `--sign-identity`.
4. `packaging/notarize_macos.sh` accepting the archive, verifying Developer ID
   signatures, submitting for notarization, and passing Gatekeeper assessment.

The public gate is available as
`just release-public-gate dist/qwe-VERSION-TARGET.tar.gz` after setting
`QWE_DEVELOPER_ID` and `QWE_NOTARY_PROFILE`.

No public-trust claim is valid until the selected Developer ID, notary
submission, and Gatekeeper checks pass. The current environment has no public
Developer ID identity available.

## Stage 5 Remaining Gates

- Run `just release-gate` on Fedora Linux x86_64 and record target-specific
  release thresholds and evidence.
- Run full manual Neovim, htop, detach/attach, and job-control release
  acceptance on both supported targets.
- Sign and notarize the selected macOS release artifact with a public
  Developer ID and verify public-distribution trust.
- Complete the final POC/manual audit disposition without rewriting failed
  evidence.
- Freeze V0 scope and change the supported-platform matrix from in progress to
  qualified only after every required gate passes.
