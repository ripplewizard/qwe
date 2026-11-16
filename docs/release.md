# V0 Release

Status: Linux-first V0 released; v0.2.5 release candidate; public macOS
deferred post-V0
Date: 2026-11-06
Updated: 2026-11-16

## Supported Platform Matrix

V0 qualification currently targets these native development platforms:

| Target | Status | Service manager | Resource enforcement | Credential provider |
|---|---|---|---|---|
| macOS ARM64 | Experimental local self-signed only; public release deferred post-V0 | launchd user agent | libproc monitoring, explicitly monitored | macOS Keychain |
| Fedora Linux x86_64 | Supported for V0 | systemd user service | cgroup v2 hard limits | private credential directory and one-shot broker |
| Ubuntu 22.04 / 24.04 x86_64 | Active development and sandbox qualification; promote only after the complete release gate | systemd user service | cgroup v2 hard limits | private credential directory and one-shot broker |
| Debian 12 x86_64 | Active development and sandbox qualification; promote only after the complete release gate | systemd user service | cgroup v2 hard limits | private credential directory and one-shot broker |
| macOS x86_64 | Not qualified for V0 | unsupported claim | unsupported claim | unsupported claim |
| Linux ARM64 | Not qualified for V0 | unsupported claim | unsupported claim | unsupported claim |
| openEuler, Kylin, Rocky | Not supported or scheduled in the current milestone | unsupported claim | unsupported claim | unsupported claim |
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
   `qwe-session-exec`, `qwe-sandbox-init`, native service assets, and a release
   manifest.
5. Safe archive extraction and installation into an isolated home prefix,
   followed by package-version and service-template verification.
6. In-place reinstall with SQLite preservation, qwed crash/restart worker
   adoption, and SQLite v9-to-current migration.
7. A release-mode benchmark with eight independent Sessions, rotating
   exclusive Attach, bounded memory sampling, and a sustained soak period.
8. On Linux, a real installed-systemd probe covering binary reinstall,
   supervisor SIGKILL, automatic restart, worker/task adoption, and graceful
   persistence, followed by Neovim/htop TUI smoke.

The local gate generates runtime artifacts under ignored `dist/`. Structured
results may be summarized in a checked-in experiment; raw terminal captures,
SQLite state, logs, and environment dumps remain local.

## Package Contract

The release directory and archive use:

```text
qwe-VERSION-TARGET[-local]/
  README.md
  INSTALL.md
  SHA256SUMS
  release.json
  install.py
  bin/
    qwe
    qwed
    qwe-sandbox-init
    qwe-session-exec
    qwe-worker
  share/qwe/service/
    launchd/
    systemd/
  share/qwe/apparmor/
    qwe-bwrap
    install.sh
```

`release.json` uses schema `qwe-release/v1` and records product and IPC
protocol versions, normalized target, source revision and dirty status,
individual executable hashes, service assets, signing mode and identity, and
the distribution profile. Non-public macOS artifacts use the `-local` suffix
and always record `distribution.public_trust: false`. `SHA256SUMS` covers every
payload file except itself, including `release.json`.

The installer validates checksums before replacing executables. It does not
modify SQLite or start a service, so reinstall and binary replacement preserve
Session state for supervised restart.

## Temporary Local macOS Signing

[ADR 0019](adr/0019-temporary-macos-local-signing.md) permits a temporary
local-only macOS release path while the Developer Program account is pending.

Create a dedicated local self-signed code-signing identity:

```sh
just macos-local-identity-create
just macos-local-identity-status
```

The private key and random Keychain password stay under
`~/Library/Application Support/QWE/local-signing` with owner-only permissions.
The explicit setup command adds the `codeSign` trust setting for that local
certificate. The certificate is not an Apple Developer ID and does not establish
public trust.

Build a local self-signed artifact:

```sh
just release-package-self-signed
```

The result is named `qwe-VERSION-macos-arm64-local.tar.gz`. Its manifest records
`distribution.profile: local-self-signed` and `public_trust: false`.
`just release-gate` uses this signing mode automatically on macOS.

Remove the local identity and its trust setting when it is no longer needed:

```sh
just macos-local-identity-remove
```

This path is for the local machine and controlled testing. It is not a public
distribution tier and does not replace the public gate below.

## Post-V0 Public macOS Distribution

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

When the account is available,
[ADR 0019](adr/0019-temporary-macos-local-signing.md) defines the restoration
sequence, and [ADR 0020](adr/0020-linux-first-v0-release.md) places the work in
a separate post-V0 milestone. The public gate rejects local unsigned or
self-signed profiles before contacting Apple.

## Release Closure

- Stage 5 is complete for Linux-first V0.
- Stage 6 has a clean `v0.1.1` release candidate and passing local Fedora
  release gate. Dedicated-runner, publication, pilot, and patch-release gates
  remain open.
- `dev` is preparing [QWE 0.2.5](releases/v0.2.5.md) with expanded usage
  documentation and completed sandbox adversarial/fault testing.
- Independent manual acceptance is recorded in
  [V0 Linux Manual Acceptance](v0-linux-manual-acceptance.md).
- Linux V0 is published as supported; macOS remains local/experimental.

The POC/manual audit disposition is complete in
[Experiment 0023](experiments/0023-v0-evidence-audit.md), and the overall
closure is recorded in
[Experiment 0025](experiments/0025-stage5-linux-v0-closure.md). Public macOS
signing and notarization are post-V0 work under ADR 0020.

## Stage 6 Distribution Gate

A published Linux V0 archive must be built from a clean release commit and pass
the manual Fedora release workflow in
`.github/workflows/release-gate.yml`. The release asset is accompanied by its
SHA-256 and release notes. Its `release.json` must report the tagged commit,
`source_dirty: false`, the expected version/target, and hashes for every
packaged executable.

Local archives under ignored `dist/` are evidence only. Do not publish one when
its manifest refers to a different source commit or marks the source dirty.
Hosted Ubuntu CI is compile/test coverage and does not expand the supported
Fedora platform claim.

Stage 6 closes only after real pilot reports are triaged and a `v0.1.x` patch
release completes the same Fedora evidence path. See
[Experiment 0026](experiments/0026-stage6-adoption-baseline.md) for the current
entry state.

A version tag matching the workspace version is rebuilt and checked on a
GitHub-hosted Linux runner, then published with its SHA-256 through the
workflow token. The complete dedicated Fedora release gate remains a separate
manual `workflow_dispatch` gate.

GitHub is the only release publication target. Gitee is retained as a source
mirror and does not receive Release assets.
