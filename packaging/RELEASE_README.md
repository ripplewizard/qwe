# QWE Release Package

This package contains the QWE V0 command-line client, supervisor, Session
worker, resource-limit launcher, and native service assets.

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
status, Developer ID identity when signed, and hashes of the four executables.
