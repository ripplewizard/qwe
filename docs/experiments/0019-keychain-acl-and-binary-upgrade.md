# Experiment 0019: Local Keychain ACL and Binary Replacement

Status: Local signed ACL and binary replacement passed
Date: 2026-11-06
Environment: macOS 26.5.1 arm64 and Fedora Linux 35 x86_64

## Keychain

No public Apple Developer identity is installed. A local self-signed
code-signing certificate was successfully created and trusted for an initial
experiment. The unattended gate uses an explicit ad-hoc signature so it never
modifies system trust settings or opens a SecurityAgent authorization dialog.

The test:

- Signs a copied `qwed` binary with an explicit ad-hoc code signature.
- Stores a test generic-password item with the signed qwed path as a trusted
  ACL application.
- Runs signed qwed against the explicit temporary Keychain path.
- Verifies successful credential delivery through inherited descriptor 3.
- Locks the Keychain and verifies qwed fails closed with
  `credential_unavailable`, without a SecurityAgent password dialog.
- Unlocks the Keychain and verifies access recovers.

Security framework user interaction is disabled in qwed. A background daemon
must never solicit a user password in the middle of a control request.

Result:

```json
{"case":"CRED-003","explicit_adhoc_code_signature":"pass","keychain_acl_trusted_app":"pass","locked_keychain_fails_closed":"pass","unlocked_keychain_recovers":"pass","public_developer_id":"not-available"}
```

Public Developer ID trust and notarized distribution remain release-identity
work, not a code-path blocker for local ACL verification. The self-signed CSR
can be used once an Apple developer program identity is available.

## Binary replacement

The managed-service probe copies qwed, worker, and launcher into a test binary
directory. With a live Session:

1. Replace the qwed path atomically.
2. SIGKILL the running old qwed.
3. Let launchd/systemd restart the path.
4. Verify the surviving worker is adopted and the Session still runs.
5. Stop the service and verify worker/task cleanup and durable terminal state.

Both launchd and systemd pass. The replacement used the same build contents;
schema migration across release versions remains Stage 5 installation
acceptance.
