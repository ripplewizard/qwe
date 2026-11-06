# QWE service packaging

These files are installation assets. They are not installed or enabled by the
development build or test suite.

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

Install `qwed`, `qwe-worker`, and `qwe-session-exec` in `~/.local/bin`. Copy
the plist to
`~/Library/LaunchAgents/com.qwe.qwed.plist`, replace `REPOSITORY_ROOT` with an
absolute checkout path, and load it with:

```sh
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.qwe.qwed.plist"
```

The wrapper stores SQLite state under `~/Library/Application Support/QWE`.
Credentials use Keychain generic-password entries named
`qwe.<CREDENTIAL_NAME>`.

## Capability boundary

`qwed` reports the active service manager and credential backend through
`CONTROL_STATUS.capabilities` and `CONTROL_STATUS.credentials`. On Linux each
Session runs in a transient systemd user scope with hard cgroup v2 memory,
swap, task-count, and CPU-bandwidth controls. macOS uses libproc monitoring and
reports `monitored` enforcement with bounded polling overshoot.

Signed-build Keychain ACL behavior, locked-keychain errors, and upgrade
behavior under each real service manager still require release acceptance.
