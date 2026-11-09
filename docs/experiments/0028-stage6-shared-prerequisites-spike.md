# Experiment 0028: Stage 6.4 Shared-Prerequisites Spike

Status: Passed at disposable-spike level; real qwed multi-Pane integration not
exercised
Date: 2026-11-09
Disposition: Stage 6.5 accepted the spike recommendations in
[ADR 0021](../adr/0021-shared-prerequisites.md) on 2026-11-09.
Host: Fedora Linux 35 x86_64, cgroup v2, systemd user manager
Entry commit: `1d98fc28a129607040c486a7c54f5a21e7df07c6`

## Purpose

Stage 6.4 is a disposable design gate for
[ADR 0021](../adr/0021-shared-prerequisites.md). It does not authorize the
proposed topology in production. The spike validates one worker per Pane,
per-Pane resource scopes, multi-Pane storage, Pane-targeted Attach, a
read-only observer projection, recovery, and cleanup.

No production topology or protocol code was added. The spike starts the
unmodified `qwed` and `qwe-worker` binaries and uses a spike-local supervisor
to group distinct worker identities under one logical Session.

## Procedure

Run:

```sh
just stage6-shared-prerequisites-spike 4
```

The harness executes four phases:

1. Use production `qwed` to create a real schema-v10 Session, Pane, Task, and
   resource policy; stop the server; back up the SQLite store.
2. Apply a prototype schema-v11 migration with Principal, ResourceScope,
   Pane-worker, and channel-grant records; preserve the V0 records and insert
   additional Panes.
3. Start four unmodified workers in a fresh process, then discover and adopt
   them from a second supervisor process.
4. Verify per-Pane terminal and journal isolation, cgroup v2 effective policy
   and usage, targeted Attach, read-only observation, a `pids.max` enforcement
   event, worker-crash isolation, process cleanup, and scope cleanup.

Production V0 rejects `read_only` Attach with `unsupported_mode`. The spike
therefore uses a qwed-owned bridge that holds the exclusive worker channel and
exposes only snapshot/output to the observer. The observer's `INPUT` attempt
is rejected and does not reach the shell.

## Completion Audit

| Stage 6.4 requirement | Status | Boundary |
|---|---|---|
| 2-4 Pane workers in one logical Session | Complete in spike | Two- and four-Pane runs passed |
| Per-Pane PTY, VT parser, scrollback, journal, and ResourceScope | Complete in spike | Worker-local screen and journal markers were isolated; each Pane had a distinct cgroup. Scrollback is worker-local by construction but has no public inspection API |
| qwed discovery, adoption, health, and cleanup | Prototype only | A spike-local supervisor performed discovery, adoption, health, crash isolation, and cleanup. The production qwed multi-Pane inventory path was not exercised |
| Per-Pane Linux cgroup v2 scope lifecycle | Complete in spike | Creation, requested/effective policy, usage/events, pressure, crash, and teardown passed |
| Storage migration from one Pane per Session | Prototype only | A production-generated v10 database migrated to a v11 prototype and preserved Session/Pane/Task/policy. Full attachment authority, retention, and concurrent-writer coverage was not exercised |
| Pane-targeted Attach and read-only observer | Interface evidence only | The spike targeted one worker directly and used a qwed-owned bridge for observer projection. Production qwed Attach routing and channel persistence were not exercised |
| Process, memory, startup, recovery, and cleanup measurements | Complete in spike | Four-Pane measurements are recorded below |

The Stage 6.4 investigation is complete for the disposable topology and
resource-interface decision. The prototype-only rows are implementation work
for Stage 7, not evidence that production qwed already supports multiple
Panes.

## Results

Both two-Pane and four-Pane runs passed. The four-Pane evidence is at
`target/stage6.4/20261109T153902Z-20ed6f83/`; generated `target/` artifacts are
not committed.

| Check | Result |
|---|---|
| V0 schema-v10 database created by production `qwed` | PASS |
| V10-to-v11 migration preserved Session, Pane, Task, and resource policy | PASS |
| Four Pane rows inserted under one logical Session | PASS |
| One distinct worker per Pane | PASS |
| Runtime socket discovery after supervisor exit | PASS |
| Worker identity and Task identity adoption | PASS |
| Health `PING` for every Pane | PASS |
| Per-Pane screen and journal marker isolation | PASS |
| Four distinct cgroup v2 scopes with effective-policy readback | PASS |
| Attach targeted to one Pane | PASS |
| Native `read_only` rejected; bridged observer remained read-only | PASS |
| `pids.max` event retained as `tasks_max` | PASS |
| One worker crash left the other three Panes healthy | PASS |
| Sockets, workers, and cgroup scopes removed | PASS |
| `result.json` validated against `poc/test-spec/result.schema.json` | PASS |

The stage-specific report identifies the run as `SHARED-001`; the
schema-compatible `result.json` uses the existing `EX01` extension-boundary
case because the canonical result schema does not yet include stage-specific
`SHARED` ids.

Four-Pane measurements:

```json
{"pane_count":4,"worker_processes":4,"task_processes":4,"aggregate_rss_bytes":37548032,"startup_ms_max":37.498,"recovery_ms":3.249,"cleanup_ms":1186.788,"resource_scopes":4}
```

These are debug-build, single-host spike measurements. They are not release
benchmarks or support claims.

## Effective Resource Scopes

| Pane | `memory.max` | `memory.high` | `pids.max` | `cpu.max` |
|---|---:|---:|---:|---:|
| 0 | 134217728 | 100663296 | 32 | `20000 100000` |
| 1 | 150994944 | 117440512 | 36 | `25000 100000` |
| 2 | 167772160 | 134217728 | 40 | `30000 100000` |
| 3 | 184549376 | 150994944 | 8 | `35000 100000` |

Each Pane resolved to a distinct `qwe-session-*.scope`. The requested policy
matched `memory.max`, `memory.high`, `memory.swap.max`, `pids.max`, and
`cpu.max` on disk. Current memory and task usage plus `memory.events` and
`pids.events` were readable per Pane.

The pressure Pane reached `pids.max=8`; the kernel recorded
`pids.events.max=1`, and the worker retained `tasks_max` as its enforcement
event. The shell remained responsive because cgroup enforcement, not SIGKILL,
rejected the excess forks.

## Findings

- Per-Pane cgroups are viable on the tested Fedora/systemd host and give clean
  attribution without changing the worker's resource implementation.
- Each worker's VT screen and bounded journal contained only that Pane's
  marker; no cross-Pane terminal state was visible.
- Grouping multiple worker identities under one Session requires a supervisor
  inventory keyed by Pane. Session-keyed worker maps and socket names are not
  sufficient.
- The prototype migration needs to remove the one-Session/one-Pane and
  one-Session/one-Task uniqueness assumptions before adding Pane workers.
- Pane-targeted Attach is achievable by selecting the Pane worker before
  opening its worker channel.
- `read_only` should not be required from the worker. An observer is naturally
  a qwed channel projection over an exclusive internal worker channel.
- `SESSION_KILL` terminates the Task process group but not unrelated process
  groups left by interactive job control in the Pane cgroup. ResourceScope
  teardown must kill or stop the complete scope. The spike verified
  `cgroup.kill` as the final fallback for such Pane-local descendants.

## Recommendation

During Stage 6.5, modify ADR 0021:

1. Keep one worker per Pane and the ResourceScope/ResourceBackend boundary.
2. Make Pane-level resource scopes the first implementation target on Linux;
   Session and Task targets remain representable by the same interface.
3. Define `human_observer` as a qwed-owned read-only channel projection over
   an exclusive internal worker Attach channel.
4. Require qwed inventory and socket addressing by Pane rather than by the
   Session's former implicit single worker.
5. Make complete scope teardown, including detached process groups, a Stage 7
   acceptance requirement; process-group signaling alone is insufficient.

The spike does not decide the production migration format, channel token
encoding, Task execution semantics, or macOS scope behavior.

## Limitations

- The grouping supervisor and observer bridge are disposable spike code, not
  production qwed behavior.
- Production multi-Pane qwed discovery, adoption, health, cleanup, public
  Attach routing, and channel persistence remain unimplemented and untested.
- The schema-v11 migration is a prototype and has not exercised the full
  product database, history retention, attachment authority, or concurrent
  writers.
- Only Fedora Linux 35 x86_64 with systemd user cgroup v2 delegation was
  tested. macOS monitored semantics and other Linux distributions were not.
- The measurements use debug binaries and a mostly idle shell workload.
