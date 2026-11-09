# Installing QWE

QWE V0 is supported on Fedora Linux x86_64. See the
[supported-platform matrix](docs/release.md#supported-platform-matrix) before
installing on other systems.

## Requirements

- Fedora Linux x86_64 with a running systemd user manager
- Rust 1.85 or newer and `just` for the current source-build installation path
- Python 3 for the release package installer
- A normal user account; do not install or run QWE as root

Verify the environment:

```sh
rustc --version
cargo --version
just --version
systemctl --user is-system-running
```

## Install from a release archive

The intended supported path is a Fedora Linux x86_64 package with its published
checksum. Stage 6 artifact publication is still pending; the commands below
apply after the `v0.1.1` release exists.

```sh
version=0.1.1
base="https://github.com/ripplewizard/qwe/releases/download/v${version}"
archive="qwe-${version}-linux-x86_64.tar.gz"

curl --fail --location --remote-name "$base/$archive"
curl --fail --location --remote-name "$base/$archive.sha256"
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "qwe-${version}-linux-x86_64"
./install.py
```

The installer verifies every payload against `SHA256SUMS` before replacing
executables or service assets. It does not start or enable the service.

## Build from source

The source-build path remains available for development and review.

Clone the stable branch and check out the current V0 tag:

```sh
git clone git@github.com:ripplewizard/qwe.git
cd qwe
git checkout v0.1.1
```

Gitee mirror:

```sh
git clone git@gitee.com:uranus-yy/bianca.git qwe
```

Build the native release package:

```sh
just release-build
just release-package
```

The package is created under `dist/`:

```text
dist/qwe-0.1.1-linux-x86_64.tar.gz
dist/qwe-0.1.1-linux-x86_64/
```

Verify and install it:

```sh
shasum -a 256 dist/qwe-0.1.1-linux-x86_64.tar.gz
tar -xzf dist/qwe-0.1.1-linux-x86_64.tar.gz -C dist
cd dist/qwe-0.1.1-linux-x86_64
./install.py
```

On Fedora, use `sha256sum` instead of `shasum`.

The installer:

- verifies every payload against `SHA256SUMS`;
- installs `qwe`, `qwed`, `qwe-worker`, and `qwe-session-exec` into
  `~/.local/bin`;
- installs the systemd user unit into `~/.config/systemd/user`;
- does not start or enable the service automatically.

If `~/.local/bin` is not on `PATH`, add it in your shell configuration.

## Start the service

```sh
systemctl --user daemon-reload
systemctl --user enable --now qwe.service
systemctl --user status qwe.service
```

The service socket is:

```text
$XDG_RUNTIME_DIR/qwe/qwe.sock
```

Verify it:

```sh
qwe --socket "$XDG_RUNTIME_DIR/qwe/qwe.sock" ping
```

Expected response:

```json
{"daemon":"ok"}
```

Service state is stored under `~/.local/state/qwe/qwe.sqlite` by the installed
systemd unit.

## Upgrade

Install the new package with the same command:

```sh
cd dist/qwe-NEW-VERSION-linux-x86_64
./install.py
systemctl --user daemon-reload
```

The installer replaces binaries atomically and does not touch SQLite. For a
planned maintenance upgrade, stop the service before replacing binaries:

```sh
systemctl --user stop qwe.service
./install.py
systemctl --user start qwe.service
```

Stopping qwed gracefully closes live Sessions. Preserve unsaved work before a
planned upgrade.

## Uninstall

Disable and stop the service first:

```sh
systemctl --user disable --now qwe.service
```

Then remove the installed user files:

```sh
rm -f "$HOME/.config/systemd/user/qwe.service"
rm -f "$HOME/.local/bin/qwe" \
  "$HOME/.local/bin/qwed" \
  "$HOME/.local/bin/qwe-worker" \
  "$HOME/.local/bin/qwe-session-exec"
systemctl --user daemon-reload
```

QWE state is intentionally not removed by the installer or by these commands.
If you also want to delete Session history, review and remove
`$HOME/.local/state/qwe` yourself.

## macOS

macOS ARM64 is experimental and limited to explicitly marked local
self-signed packages. Public macOS binary distribution is deferred post-V0.
See [Packaging](packaging/README.md#temporary-macos-local-signing).

## Next

Continue with [Usage](USAGE.md). For release validation and limitations, see
[V0 Release](docs/release.md).
