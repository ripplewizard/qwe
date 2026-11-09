# QWE service packaging

These files are installation and release assets. The development test suite
does not install or enable a user service.

## Release package

Build the native release binaries and package the four executables, service
assets, checksums, and release manifest:

```sh
just release-build
just release-package
```

On macOS, unsigned builds are marked `local-unsigned` and receive a `-local`
suffix. Use the local self-signed tier described below for the temporary
release workflow.

Generated packages are written under the ignored `dist/` directory. Install
from an unpacked package with:

```sh
./install.py
```

The installer verifies `SHA256SUMS`, writes executables to `~/.local/bin`, and
writes the native service asset. It does not start or enable the service.
Use `--prefix` to test against an isolated home without touching the current
account.

## systemd user service

Install `qwed`, `qwe-worker`, and `qwe-session-exec` in `~/.local/bin`, the
systemd unit at `~/.config/systemd/user/qwe.service`, and then run:

```sh
systemctl --user daemon-reload
systemctl --user enable --now qwe.service
```

The unit uses the systemd credentials directory when the user adds an
encrypted credential entry such as:

```ini
LoadCredentialEncrypted=EXAMPLE_KEY:/home/user/.config/qwe/example-key.cred
```

`qwed` reads the resulting `$CREDENTIALS_DIRECTORY/EXAMPLE_KEY` as a private
regular file. It does not put the secret in argv, its environment, workspace
files, logs, or SQLite.

## launchd user agent

The release installer places `qwed`, `qwe`, `qwe-worker`,
`qwe-session-exec`, and the launchd wrapper in `~/.local/bin`. It renders the
installed plist at `~/Library/LaunchAgents/com.qwe.qwed.plist`; no checkout
path is required. Load it with:

```sh
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.qwe.qwed.plist"
```

The wrapper stores SQLite state under `~/Library/Application Support/QWE`.
Credentials use Keychain generic-password entries named
`qwe.<CREDENTIAL_NAME>`.

## Temporary macOS local signing

Create the machine-local self-signed identity and build the explicitly
non-public package:

```sh
just macos-local-identity-create
just release-package-self-signed
```

The output is `qwe-VERSION-macos-arm64-local.tar.gz`. The installed public
trust remains false, and the public gate rejects this profile. The identity
uses a dedicated local Keychain and can be removed with
`just macos-local-identity-remove`.

## Capability boundary

`qwed` reports the active service manager and credential backend through
`CONTROL_STATUS.capabilities` and `CONTROL_STATUS.credentials`. On Linux each
Session runs in a transient systemd user scope with hard cgroup v2 memory,
swap, task-count, and CPU-bandwidth controls. macOS uses libproc monitoring and
reports `monitored` enforcement with bounded polling overshoot.

Local signed Keychain ACL, locked-Keychain fail-closed behavior, binary
replacement, and the temporary local self-signed package tier pass. Public
Developer ID signing and notarization remain a separate strict gate:

```sh
python3 packaging/build_release.py \
  --sign-identity "$QWE_DEVELOPER_ID" --force
QWE_DEVELOPER_ID="$QWE_DEVELOPER_ID" \
QWE_NOTARY_PROFILE="$QWE_NOTARY_PROFILE" \
  packaging/notarize_macos.sh dist/qwe-0.1.1-macos-arm64.tar.gz
```

The equivalent `just` entry points are
`just release-package-signed "$QWE_DEVELOPER_ID"` and
`just release-public-gate dist/qwe-0.1.1-macos-arm64.tar.gz`.

The notarization gate fails if the identity, Apple notary profile, signatures,
notary acceptance, or Gatekeeper assessment is unavailable. It never replaces
these checks with an ad-hoc signature.

## Cross-UID security test

The cross-UID probe intentionally uses a second local account with one tightly
scoped sudoers command:

```text
huangjian ALL=(qwe-probe) NOPASSWD: /tmp/qwe-cross-uid-client.py
```

Use the corresponding current username on other hosts. The helper only runs as
the unprivileged `qwe-probe` account. Do not grant `NOPASSWD: ALL`.

The probe uses an isolated runtime directory with owner-only directory and
socket permissions rather than installing these assets. Packaged installation,
upgrade migration, and public distribution signing remain Stage 5 acceptance.
