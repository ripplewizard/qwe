# Using QWE

QWE provides durable terminal Sessions supervised by `qwed`. A Session owns one
PTY, one task instance, terminal state, process-group teardown, and persisted
history.

## Connect to the service

The default CLI socket is intended for development. An installed Linux service
uses the runtime directory socket:

```sh
export QWE_SOCKET="${XDG_RUNTIME_DIR}/qwe/qwe.sock"
```

Use it on every command:

```sh
qwe --socket "$QWE_SOCKET" status
qwe --socket "$QWE_SOCKET" ping
```

Responses are JSON. Successful requests exit with status 0; failures exit
nonzero with a stable error code and message on stderr.

## Create and attach to a Session

Create a shell Session:

```sh
qwe --socket "$QWE_SOCKET" session create demo sh
```

The response includes `session_id`, `task_id`, `pane_id`, process identities,
and worker information.

Attach interactively:

```sh
qwe --socket "$QWE_SOCKET" session attach demo
```

Detach without stopping the Session by pressing:

```text
Ctrl-]
```

Reattach later:

```sh
qwe --socket "$QWE_SOCKET" session attach demo
```

The terminal snapshot, cursor, terminal modes, and live output offset are
restored. A second active owner cannot attach while the Session is exclusively
attached.

## Inspect Sessions

```sh
qwe --socket "$QWE_SOCKET" status
qwe --socket "$QWE_SOCKET" session status demo
qwe --socket "$QWE_SOCKET" session snapshot demo
qwe --socket "$QWE_SOCKET" task history
```

`session snapshot` is diagnostic. It returns structured terminal state plus a
bounded journal tail; it is not an unbounded replay format.

## Send input without an interactive attach

The CLI creates a temporary Active attachment:

```sh
qwe --socket "$QWE_SOCKET" session input demo $'printf "hello\\n"\r'
```

Use `\r` for Enter in the child PTY.

## Ownership and Leases

The default owner is `local`. Control operations can require the current
attachment or Lease authority:

```sh
qwe --socket "$QWE_SOCKET" lease claim demo alice --ttl-ms 60000
qwe --socket "$QWE_SOCKET" lease status demo
qwe --socket "$QWE_SOCKET" --owner alice session close demo --generation 1
```

Lease generations fence stale owners. Releasing or letting a Lease expire does
not terminate the Session or Task.

## Resources

Resource policies apply to the whole Session process tree. Linux uses a
transient systemd cgroup v2 scope:

```sh
qwe --socket "$QWE_SOCKET" session create limited \
  --memory-max 268435456 \
  --cpu-seconds 60 \
  --tasks-max 64 \
  sh
```

Change limits on a running held Session:

```sh
qwe --socket "$QWE_SOCKET" --owner alice session limits demo \
  --generation 1 \
  --memory-max 536870912
```

Clear a limit:

```sh
qwe --socket "$QWE_SOCKET" --owner alice session limits demo \
  --generation 1 \
  --clear
```

Consult `qwe session create --help` and `CONTROL_STATUS.capabilities` for
supported fields. Unsupported requests return `unsupported_capability`; they
are never silently ignored.

## Credentials

The Linux service can use systemd credentials. Create a Session with a named
credential:

```sh
qwe --socket "$QWE_SOCKET" session create api-job \
  --credential EXAMPLE_KEY \
  sh
```

The secret is delivered through inherited file descriptor 3. The child sees
`QWE_CREDENTIAL_FD=3`; secret bytes do not appear in argv, the environment,
database, logs, or terminal output.

Credential setup is documented in [Packaging](packaging/README.md).

## Stop and teardown

Stop one Task while retaining terminal history:

```sh
qwe --socket "$QWE_SOCKET" task stop TASK_ID \
  --owner alice \
  --generation 1
```

Close one Session gracefully:

```sh
qwe --socket "$QWE_SOCKET" --owner alice session close demo --generation 1
```

Force-kill a Session:

```sh
qwe --socket "$QWE_SOCKET" kill-session demo --generation 1
```

Force-kill the server and every Session:

```sh
qwe --socket "$QWE_SOCKET" kill-server --confirm
```

Forced teardown kills real process groups and preserves durable terminal
history.

## Detach and job control

Interactive shells retain job-control state across detach/reattach:

```sh
sleep 300
# Ctrl-Z
jobs
bg
jobs
fg
# Ctrl-C
```

Terminal modes and Kitty keyboard state are reset on detach so the local shell
does not receive leaked key-release sequences.

## Troubleshooting

Check the service:

```sh
systemctl --user status qwe.service
journalctl --user -u qwe.service -n 100 --no-pager
```

Check the socket and versions:

```sh
ls -l "$XDG_RUNTIME_DIR/qwe/qwe.sock"
qwe --version
qwed --version
```

Socket connection failures usually mean the service is stopped, the socket path
is wrong, or the process is running under a different user.

## Development

From the repository:

```sh
just check-qwe
just stage4-check
just release-gate
```

See the [Development Workflow](docs/development-workflow.md) and
[Release](docs/release.md) for branch, evidence, and platform rules.
