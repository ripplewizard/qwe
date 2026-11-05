# Experiment 0005: Stage 2 Attach Channel

Status: Automated and independent manual acceptance passed on Linux and macOS
Date: 2026-11-05
Platform: Linux x86_64 and macOS ARM64

## Scope

This experiment adds the end-to-end Stage 2 data plane. After the Stage 1
cutover the path is:

```text
qwe CLI -> qwed Attach socket -> worker Attach socket
```

Control requests and snapshot diagnostics remain independent of this channel.

## Implemented

- Dedicated owner-only Attach sockets beside the qwed, Host, and worker control
  sockets.
- Canonical typed frame read/write helpers and payload types.
- `ATTACH_OPEN`, `ATTACH_ACCEPTED`, `SNAPSHOT`, raw `OUTPUT`, raw `INPUT`,
  `RESIZE`/`RESIZED`, `ACK`, `DETACH`, `PROCESS_EXITED`, `OWNERSHIP_REVOKED`,
  and `ERROR`.
- Subscriber registration before snapshot capture and offsets that continue
  exactly at the snapshot's `next_stream_offset`.
- One bounded 1 MiB output accounting queue per attachment. Overflow closes
  only that attachment with `output_backpressure`.
- Worker-owned structured VT state: size, cursor position/style/visibility,
  alternate screen, application cursor/keypad, bracketed paste, and mouse
  protocol mode/encoding.
- PTY resize applied under the same stream lock that orders `RESIZED` against
  output offsets.
- Exclusive Attach rejection, EOF release, explicit immediate detach, and
  reconnect with a fresh snapshot.
- Interactive CLI raw mode, input forwarding, terminal-size polling, snapshot
  rendering, `Ctrl-]` detach, and terminal restoration.
- Independent manual T03/T04/T08/T11 review passed on macOS and Fedora; see
  [Experiment 0010](0010-stage-2-manual-tui-signoff.md).
- `read_only` returns `unsupported_mode`; positive-TTL holds are deferred and
  also report `unsupported_mode`.

## Verification

- `cargo test --workspace`: pass.
- `just attach-e2e`: pass three consecutive times on each platform with real
  sockets, PTYs, slow-client backpressure, and processes.
- `just tui-attach-smoke`: pass on both platforms with tmux-driven Neovim and
  htop detach, reattach, screen restoration, and process exit.
- `just stage1-regression-smoke`: pass.
- `just t12-host-smoke`: pass; Host plus worker RSS was 45,392 KiB on macOS and
  35,280 KiB on Linux.
- `just resilience-smoke`: pass on both platforms; 64/64 concurrent snapshot
  requests succeeded. macOS P95/P99 was 349/389 ms; Linux P95/P99 was
  736/798 ms, both below the 3,000/5,000 ms gates.

The channel probe covers the attachment portion of `ATTACH-001` through
`ATTACH-011`, and `ATTACH-013` through `ATTACH-014` on both platforms.
`ATTACH-008` verifies isolated `output_backpressure` close followed by a
successful reattach. `ATTACH-012` queue isolation is covered at unit level
because V0 exclusive Attach does not permit a second live client on the same
Session.

## Deliberate Limits

- The supervisor consolidation and object-model cutover remain Stage 1 work.
- These results describe the Stage 2 cutover; later Stage 3 work adds durable
  authority, reclaim grace, and detached holds.
- iTerm2 retains a separate known Kitty-keyboard-protocol restoration defect;
  Terminal.app and Fedora passed the manual matrix.
- Raw terminal captures and runtime databases remain outside the repository.
