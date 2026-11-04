# Experiment 0002: Resilience and Concurrent Attach

Status: Passed
Date: 2026-11-03

## Questions

1. Can a Session survive `qwe-host` being killed with its process group?
2. Can SQLite restore Lease, Task, and reconciliation state after both control
   processes restart?
3. Does Attach remain bounded and responsive with concurrent clients during
   continuous PTY output?

## Setup

`just resilience-smoke` uses real processes and a real SQLite file:

- A Task starts a Shell Session in an isolated PTY worker.
- A lease is claimed and the original worker/shell PID is recorded.
- `qwe-host` is killed with `SIGKILL` while `qwed` remains live; the old
  control plane must detect the new epoch and reconnect to the recovered Host.
- `qwed` is then killed and restarted separately to exercise SQLite recovery.
- The worker must remain alive across both control-plane failures.
- SQLite must restore the running Task and Lease, and Host inventory must
  reconcile without marking the live Session lost.
- A separate Task's worker is killed while `qwed` remains live; the runtime
  monitor must persist that Session and Task as `lost` before restart.
- A continuous-output process runs while eight clients issue 64 Attach requests.
- Closing and duplicate-daemon behavior are also checked.

## Result

The test passed on macOS ARM64 and Fedora Linux x86_64. Three consecutive
benchmark runs on each platform produced:

| Platform | P50 | P95 | P99 | Successful requests |
|---|---:|---:|---:|---:|
| macOS ARM64 | 64.1-90.0 ms | 137.1-227.4 ms | 141.6-240.5 ms | 64/64 per run |
| Fedora x86_64 | 301.0-713.5 ms | 682.1-847.5 ms | 750.9-866.7 ms | 64/64 per run |

The latency gate is P95 <= 3,000 ms and P99 <= 5,000 ms.

The original Session PID and start token survived Host and `qwed` restarts.
The pre-crash screen and input history remained available. Lease owner and
generation survived, Task history moved from `running` to `stopped`, and the
killed worker was reconciled as `lost` within 250 ms without restarting
`qwed`. Releasing a Lease wrote a generation tombstone; the next claim
continued from the next generation.

## Proven

- Host crash recovery through isolated session workers.
- SQLite schema v2 persistence for Session, Task, process identity, exit status,
  and Lease state.
- Startup reconciliation of Host inventory before serving clients.
- Concurrent Attach under sustained output with measured tail latency.

The 100 MiB bounded-output suite kept Host plus PTY-worker RSS below 200 MiB:
44,896-45,024 KiB on macOS and 28,840-31,860 KiB on Fedora.

## Remaining Limits

- OS reboot and machine migration are not recovered.
- Worker crash loses the affected Session, by design.
- The benchmark runs on development builds; release measurements remain a CI
  reporting task.
- Read-only observers, terminal resize, and long-duration soak testing are not
  covered here.
