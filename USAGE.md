# Using QWE

QWE provides durable terminal Sessions supervised by `qwed`. A Session groups
one or more Pane workers; each Pane owns a PTY, task instance, terminal state,
resource scope, process-group teardown, and persisted history.

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

## Compose multiple Panes

Create another Pane under a leased Session:

```sh
qwe --socket "$QWE_SOCKET" --owner alice lease claim demo alice --ttl-ms 60000
qwe --socket "$QWE_SOCKET" --owner alice pane create demo sh \
  --generation 1
```

Inspect the current layout:

```sh
qwe --socket "$QWE_SOCKET" session layout demo
```

Open the compositor:

```sh
qwe --socket "$QWE_SOCKET" session compose demo
```

`Ctrl-A n` and `Ctrl-A p` move focus forward and backward. `Ctrl-A 1` through
`Ctrl-A 9` select a Pane directly. `Ctrl-A ]`, `Ctrl-A q`, or `Ctrl-]` detach
all Pane channels. Focus and Pane order survive detach and qwed restart.

Inside the compositor:

- `Ctrl-A c` creates and focuses a shell Pane immediately after the current
  Pane.
- `Ctrl-A x` closes the focused Pane; the last visible Pane must be closed with
  its Session.
- `Ctrl-A h` and `Ctrl-A l` move the focused Pane left or right.
- `Ctrl-A f` creates and focuses a fixed floating shell Pane.
- `Ctrl-A d` enters keyboard move mode for a focused floating Pane.
- `Ctrl-A [` enters copy mode for the focused Pane's visible screen.
- `Ctrl-A m` opens the client-only quick menu.

In copy mode, `h/j/k/l` moves, Space or `v` starts selection, `V` selects the
current row, `A` selects the visible screen, and `y` or Enter copies. `q` or
Escape cancels. Clipboard success names the provider; missing or failed
clipboard providers return an explicit `copy failed` status.

`Ctrl-U` and `Ctrl-D` scroll copy mode through the focused Pane's bounded
client scrollback. Mouse press/motion/release inside copy mode selects text in
that same Pane and copies on release.

The quick menu accepts `c`, `f`, `d`, `x`, `h`, `l`, `[`, and `q` without sending
those keys to the PTY. Copy currently covers the
focused Pane's visible screen, not scrollback.

Create a floating Pane without a shortcut:

```sh
qwe --socket "$QWE_SOCKET" --owner alice pane create demo sh --kind floating
```

Move and resize a floating Pane explicitly:

```sh
qwe --socket "$QWE_SOCKET" --owner alice pane move PANE_ID \
  --x 8 --y 4 --rows 21 --columns 81
```

In floating move mode, `h/j/k/l` moves the popup, `H/L` changes width, `K/J`
changes height, `y` saves, and `q` or Escape cancels.

Drag a floating Pane by holding the left mouse button on its title row and
releasing at the destination. Content-area mouse events are forwarded to the
Pane when its terminal application enabled mouse reporting. The compositor
tracks mouse buttons with SGR encoding while attached.

Drag the visible bottom-right `+` marker to resize a floating Pane. Width and
height are clamped to the usable viewport and saved once on release.

Left-click a floating Pane to raise it above other overlays. Raise z-order also
survives detach and qwed restart.

Persist a focused Pane without entering the compositor:

```sh
qwe --socket "$QWE_SOCKET" --owner alice pane focus PANE_ID --generation 1
```

Stage 8 slices 1-10 use deterministic tiling derived from the current terminal
size. Schemas v12/v13 persist Pane order, z-index, focus, and floating bounds.
Split and reorder controls are implemented with equal/default ratios; arbitrary
ratios, scrollback/mouse copy, and floating-Pane dragging remain future work.

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
