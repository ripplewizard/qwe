# QWE Release Package

This package contains the QWE V0 command-line client, supervisor, Session
worker, resource-limit launcher, and native service assets.

Packages whose names end in `-local` are not eligible for public macOS
distribution. Check `release.json` for `distribution.profile` and
`distribution.public_trust`; local unsigned and self-signed packages always
report `false`.

Install into the current user account:

```sh
./install.py
```

Install into a controlled prefix without touching the current service-manager
configuration:

```sh
./install.py --prefix /path/to/test-home
```

The installer verifies `SHA256SUMS`, installs binaries under
`.local/bin`, and writes native service assets under the selected prefix.
It does not start or enable a service. See `INSTALL.md` for service-manager and
credential setup.

Every release archive is accompanied by `release.json`, which records the
product version, IPC protocol version, target, source revision, dirty-source
status, signing mode and identity, distribution profile and public-trust
status, and hashes of the four executables.
