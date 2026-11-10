# Experiment 0029: Stage 7 Multi-Pane, Pressure, and Qualification

Date: 2026-11-10
Environment: Fedora Linux 35 x86_64, kernel 5.14.10, systemd user manager
running, cgroup v2 with `cpu`, `memory`, and `pids`.

## Scope

Validate the Stage 7 implementation added after the first resource-policy
slice: schema v11, Pane-keyed inventory/recovery, multi-Pane teardown,
concurrent pressure, deterministic soak, installed-systemd service behavior,
and the Linux qualification matrix.

## Commands

```sh
just check-qwe
just stage7-resource-profile
just stage7-multi-pane
just stage7-pressure
just stage7-soak-short
just service-lifecycle-smoke
python3 tests/fixtures/stage7_node_gate.py --soak-seconds 10
python3 scripts/stage7_matrix.py --soak-seconds 10
just stage7-soak 5 16 2 30
```

## Results

- Schema v11 migration passed for a populated v10 database, including a
  verified backup, stable Session/Pane/Task identities, attachment token,
  requested policy, idempotency replay, and lease preservation.
- Production qwed created three Pane workers under one Session, recovered all
  three after qwed SIGKILL, and isolated a later single-worker crash.
- RG-004 ran 362 qwed PING samples while memory OOM, CPU-seconds, and
  `pids.max` pressure were active. PING p95 was 1.249 ms and max was 1.664 ms;
  the control Pane remained alive.
- The operation-dense 20-second soak ran 8 Sessions with 2 Panes each, 3 qwed
  restarts, 5 churn Panes, 768 Attach/snapshot samples, 192 each of resize,
  input, Pane snapshot, and PING operations, 24 each of Task and Lease status,
  and 2 resource-policy updates. Attach p95 was 59.586 ms and p99 was
  69.580 ms; all scopes were cleaned.
- The installed systemd service passed binary replacement, qwed SIGKILL
  recovery, worker adoption, graceful cleanup, and Pane-keyed SQLite checks.
- Fedora passed the RG-005 node gate. Ubuntu 22.04, Ubuntu 24.04, and Debian
  12 were recorded as `blocked` because no real-VM SSH targets were available.
  Rootless containers were not used as qualification evidence.
- No `qwe-pane-*.scope` remained after deterministic gates.
- A qualification attempt exposed read-only requests being persisted as
  idempotency records. With keyed snapshots this grew SQLite to 262 GiB and
  caused the next qwed restart to fail. Read-only operations now bypass the
  idempotency transaction. The regression probe sends 1800 keyed read requests,
  leaves only 2 mutation records, keeps SQLite at 4096 bytes, and passes three
  qwed SIGKILL/restart adoption cycles.

The release-mode qualification soak is bounded to five hours with 16 Sessions
x 2 Panes and a qwed restart every 30 minutes. The runner also exercises
resize, input, Pane snapshots, control/task history, Lease inspection and
touch, resource-policy updates, and Pane churn. It writes durable qwed logs,
`progress.json`, and `result.json` beneath `target/stage7-soak/`.

## Follow-up

- Provide reachable Ubuntu 22.04, Ubuntu 24.04, and Debian 12 real-VM targets
  and rerun `stage7-qualification-matrix`.
- Collect and review the five-hour soak JSON and logs.
