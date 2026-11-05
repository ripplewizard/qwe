# Experiment 0006: Stage 1 Control Plane

Status: Passed on Linux x86_64 and macOS ARM64
Date: 2026-11-05
Code commit: `b7b47eb`

## Scope

This experiment implements the three Stage 1 gates:

1. Consolidate worker supervision into qwed and remove the Host runtime path.
2. Replace polling liveness with event-first detection, persistent control
   channels, and a bounded safety scan.
3. Persist explicit Session 1:1 Pane 1:1 TaskInstance cardinality and remove
   the duplicate `task.start` creation path.

## Implemented

- qwed directly spawns and re-adopts `qwe-worker` processes.
- `qwe-host`, Host epoch fencing, `host_epoch`, inventory polling, and
  `TASK_START` are removed from the production runtime.
- qwed holds a process-lifetime `flock(2)` on `qwed.lock`.
- A self-pipe `sigaction(SIGCHLD)` monitor calls `waitpid(WNOHANG)` and emits
  worker-exit events.
- Each worker has separate persistent normal-control and health-control
  connections. Control requests support multiple in-flight request IDs.
- Persistent-control EOF marks a live worker `unreachable`; a two-second safety
  scan probes with a 500 ms deadline. Calls to one worker do not hold a global
  supervisor lock.
- SQLite schema v5 adds `panes`, unique `tasks.session_id`, and persisted worker
  identity fields. `session.create` atomically creates exactly one Session,
  Pane, and TaskInstance.
- The CLI and protocol expose no `task.start` creation path.
- qwed Attach proxies directly to the worker Attach socket after the Stage 1
  cutover.

## Validation

The final check ran concurrently through:

```sh
pdsh -w localhost,istoria -f 2 \
  "just check-qwe && just stage1-check && just attach-e2e && just tui-attach-smoke"
```

Both hosts passed `just check-qwe`, all Stage 1 gates, Attach E2E, and the
tmux-driven Neovim/htop smoke.

| Platform | Worker-loss event | T12 qwed + worker RSS | Concurrent Attach P95 / P99 |
|---|---:|---:|---:|
| macOS ARM64 | 49 ms | 46,800 KiB | 1,160 / 1,338 ms |
| Fedora Linux x86_64 | 35 ms | 37,216 KiB | 909 / 931 ms |

The Stage 1 regression also verifies:

- A `SIGSTOP` worker becomes `unreachable` without delaying qwed PING or a
  different healthy Session.
- The second qwed fails while the first holds the runtime lock.
- qwed SIGKILL leaves the worker alive; restart re-adopts it and restores the
  final terminal state.
- Duplicate TaskInstance insertion fails and every live Session has one Pane.
- Transaction rollback, terminal SessionId reuse, task-stop semantics, and
  process-group close behavior remain correct.

## Remaining Limits

- Complete orphan/conflict reconciliation matrix coverage remains Stage 3.
- Attachment authorization, reclaim tokens, detached holds, and durable
  idempotency remain Stage 3.
- Independent manual Neovim/htop sign-off remains open even though automated
  TUI smoke passes on both platforms.
- Raw terminal captures and runtime SQLite databases remain local.
