# ADR 0019: Temporary Local macOS Signing

Status: Accepted temporarily
Date: 2026-11-06

## Context

Public macOS distribution requires an Apple-issued Developer ID Application
identity and Apple notarization. A Developer Program account is not currently
available. Removing macOS support would block Stage 5 work on the existing
macOS development and release path, while an unsigned binary would make local
release identity and integrity ambiguous.

## Decision

Until an Apple Developer Program account and Developer ID identity are
available:

- macOS release candidates use a locally generated, locally trusted
  self-signed code-signing certificate.
- The private key remains in a dedicated local Keychain under the user's QWE
  application-support directory. It is never exported into the repository or
  a distributed package.
- Self-signed artifacts carry a `-local` package-name suffix.
- `release.json` records `signing_mode: self-signed`,
  `distribution.profile: local-self-signed`, `distribution.public_trust:
  false`, and `distribution.notarized: false`.
- The macOS `just release-gate` uses the local identity and produces a local
  artifact. It does not claim Gatekeeper public trust.
- The public notarization gate rejects every profile except
  `developer-id-unnotarized`; it therefore rejects local self-signed packages
  before contacting Apple.
- The supported-platform matrix marks macOS public binary distribution as
  unavailable until the Developer ID gates pass.
- Local self-signed signing remains a development and controlled-distribution
  mechanism. It is not a substitute for Developer ID, notarization, or public
  trust.

## Restoration

After Apple approves the account:

1. Install the Developer ID Application certificate and private key.
2. Create `QWE_NOTARY_PROFILE` with `notarytool`.
3. Build with `just release-package-signed "$QWE_DEVELOPER_ID"` so the artifact
   drops the `-local` suffix and records `developer-id-unnotarized`.
4. Run `just release-public-gate` and require notary acceptance plus
   Gatekeeper assessment.
5. Record the public-trust result, update the platform matrix, and remove or
   supersede this temporary decision.

The packaged installer and runtime capability behavior do not change during
restoration.

## Consequences

- Stage 5 package, upgrade, migration, and performance work can continue on
  macOS without Apple credentials.
- Local Keychain certificate trust is modified only by the explicit local
  identity setup command and can be removed with the corresponding cleanup
  command.
- Self-signed builds retain stable local identity and verifiable binary
  integrity for controlled users.
- Users outside the explicitly configured local trust boundary may see
  Gatekeeper rejection. That limitation must remain visible and must not be
  described as supported public distribution.
