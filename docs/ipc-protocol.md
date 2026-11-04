# QWE IPC Protocol

Status: V0 Draft
Date: 2026-11-04

This document describes the currently implemented request/response protocol.
[ADR 0012](adr/0012-terminal-attach-data-plane.md) and
[ADR 0013](adr/0013-core-object-model.md) propose the long-lived Attach channel
and authoritative object semantics that will replace parts of this contract
after acceptance.

## 1. Authority Boundaries

- `qwed` is the sole authority for SQLite Sessions, Tasks, and Leases.
- `qwe-host` is the authority for live worker inventory and PTY/process health.
- Session workers own PTYs and report resource state through Host IPC.
- Clients never connect directly to workers in V0.

## 2. Transport

All links use SOCK_STREAM Unix Domain Sockets. Messages are a four-byte
big-endian length followed by JSON. Frames are limited to 8 MiB. Sockets are
mode `0600`; every server also verifies peer UID.

Request fields:

```json
{
  "version": 1,
  "request_id": "unique-id",
  "idempotency_key": "optional",
  "host_epoch": 7,
  "operation": "SESSION_STATUS",
  "payload": {}
}
```

## 3. qwed to qwe-host

`HOST_REGISTER` requests a new monotonic fencing epoch:

```json
{
  "epoch": 7,
  "protocol": 1,
  "recovered_sessions": 2
}
```

`HOST_LIST` returns the live worker inventory:

```json
{
  "epoch": 7,
  "sessions": [
    {
      "session_id": "build",
      "worker_pid": 123,
      "worker_identity": {
        "pid": 123,
        "pgid": 123,
        "start_token": "boot:ticks"
      },
      "process_identity": {
        "pid": 124,
        "pgid": 124,
        "start_token": "boot:ticks"
      },
      "running": true
    }
  ]
}
```

Supported operations:

```text
PING
HOST_LIST
SESSION_CREATE
SESSION_STATUS
SESSION_ATTACH
SESSION_INPUT
SESSION_DETACH
SESSION_CLOSE
```

Only `HOST_REGISTER` omits `host_epoch`. The Host rejects stale epochs.

## 4. Runtime Reconciliation

`qwed` reconciles Host inventory every 250 ms while running. Host inventory
checks every worker; an unavailable worker is removed and reported as
`terminal_lost`. Reconciliation then marks missing running Sessions and Tasks
as `lost`.

Host restart scans its private runtime directory. A worker is accepted only
when:

- The socket path matches the expected session-derived path.
- `session_id` matches.
- Worker PID and PID start token match the live process identity.

## 5. Session Worker Protocol

Worker operations are `WORKER_HELLO`, `SESSION_STATUS`, `SESSION_ATTACH`,
`SESSION_INPUT`, and `SESSION_CLOSE`.

`SESSION_ATTACH` returns a bounded screen snapshot and at most the newest
1 MiB journal tail. The journal tail is opaque raw bytes; the screen snapshot
is the authoritative display. A future compact VT protocol will make all
cursor and terminal modes explicit.

## 6. Lease State Machine

Leases use UTC wall-clock milliseconds and these states:

- Active: owner and deadline are set.
- Expired: deadline is not in the future; ownership operations require
  claim/takeover.
- Released: owner is empty and `released_at_ms` is set; generation remains as a
  tombstone.

`owner` is an application-level client identity, not a PID. Expiry never kills
a Task or Session. Generation increments on takeover and release, so accepted
operations cannot reuse an older fencing token.

## 7. Backpressure

Each Session has one bounded 16 MiB journal. Attach takes a private snapshot of
at most 1 MiB; slow clients write only their own response and do not hold the
PTY reader or other clients' writers. A blocked client may consume one bounded
response buffer until its socket timeout or disconnect.

## 8. Security Model

Owner-only sockets prevent other UIDs from connecting at the OS layer.
Application peer-UID checks remain defense in depth for misconfigured paths,
future group-accessible sockets, and compromised file permissions.
