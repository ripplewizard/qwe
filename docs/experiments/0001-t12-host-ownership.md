# Experiment 0001: Durable Host Ownership

Status: Passed
Date: 2026-11-03

## Question

Can terminal sessions survive a control-daemon crash?

## Setup

- `qwe-host` owns a real PTY and Shell session.
- `qwed` proxies versioned client requests to the host.
- The CLI creates a session and writes `QWE_T12_BEFORE`.
- `qwed` is killed with `SIGKILL` while the host stays alive.
- A new `qwed` starts against the same host socket.
- The CLI attaches and writes `QWE_T12_AFTER`.

## Result

`just t12-host-smoke` passes:

- Host process remains alive after `qwed` death.
- Session PID remains unchanged.
- The pre-crash screen is recovered.
- Input remains writable after reconnect.
- New output is visible through the restarted control plane.
- A 100 MiB continuous-output run is polled through lightweight status requests.
- The final attach is limited to a 1 MiB journal tail with explicit truncation.
- `QWE_BOUNDED_END`, written after all 100 MiB, is present in the retained tail.
- Host plus PTY-worker RSS stayed below 200 MiB: 44,896-45,024 KiB on macOS and
  28,840-31,860 KiB on Fedora.

Validated on macOS ARM64 and Fedora Linux x86_64.

## Proven

A dedicated host removes PTY and terminal state from the daemon lifetime.
The host also keeps output retention bounded while continuing to drain the PTY.

## Covered by Experiment 0002

- Host crash recovery, Lease/Task persistence, startup reconciliation, and
  concurrent Attach.

## Not Yet Proven

- Rejection of adversarial unauthorized local clients.
- Compact binary VT snapshot transport.
