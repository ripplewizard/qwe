# Storage Migration: Shared-Prerequisites Schema v11

Status: Accepted design target; implementation pending Stage 7
Date: 2026-11-09
Authority: [ADR 0021](adr/0021-shared-prerequisites.md) and the current v10
schema in `crates/qwe-storage`.

## 1. Purpose

Schema v11 removes the V0 one-Pane-per-Session and one-Task-per-Session
assumptions while preserving every valid v10 record. It introduces durable
Pane-worker, ResourceScope, and Principal records that the accepted shared
prerequisites require.

This plan freezes migration invariants, not Rust types or final SQL. Stage 7
may adjust physical details only when the invariants and compatibility rules
remain intact.

## 2. Current v10 Facts

- `sessions` stores the optional worker PID, PGID, and start token.
- `panes.session_id` is unique, so a Session cannot own multiple Panes.
- `tasks(session_id)` has a unique index, so a Session cannot own multiple
  Tasks.
- Pane resource policy is stored as columns on `panes`.
- Lease state is Session-scoped.
- Attachment authority is keyed by `session_id`.
- Idempotency records and task history are durable and must survive migration.
- `PRAGMA user_version = 10`; older binaries reject a newer schema.

## 3. Target Invariants

```text
Session
  +-- Pane
        +-- Task
```

- A Session may own multiple Panes.
- A Pane belongs to exactly one Session.
- A Pane has one worker identity and one owned PTY/process tree.
- A Task belongs to exactly one Pane.
- qwed inventory and recovery are keyed by Pane.
- Session-wide Lease authority remains Session-scoped.
- Interactive and observer attachment authority is Pane-scoped.
- No public protocol field exposes cgroup paths, systemd unit names, sockets,
  AuthContext internals, or process details.

## 4. Schema Shape

The following names describe the required logical records. Exact column order
and indexes are settled with the Stage 7 migration implementation.

### `panes`

- Keep `id` as the stable Pane identity.
- Keep `session_id` and add a non-unique `(session_id, id)` lookup index.
- Remove authoritative resource-policy columns after their values move to
  ResourceScope records.

### `pane_workers`

- `pane_id` is the primary key and foreign key to `panes`.
- Store worker PID, PGID, start token, lifecycle state, and update time.
- Move worker identity out of `sessions`.
- Runtime socket paths are derived from Pane identity; inventory is not keyed
  by the Session's former implicit single worker.

### `tasks`

- Replace the single `session_id` parent with `pane_id`.
- Remove the unique one-Task-per-Session index.
- Session queries derive their Session through the owning Pane.
- Whether a Pane may run multiple live Tasks at once remains an Agent
  Execution decision; the storage schema must not encode an accidental
  one-Task cap.

### `resource_scopes`

- Stable opaque `id`.
- `target_kind` (`session`, `pane`, or `task`) and opaque `target_id`.
- Optional `parent_scope_id`.
- Backend kind, requested policy, lifecycle state, and cleanup tombstone.
- Current/peak usage and effective policy remain runtime readback. Only
  bounded enforcement events and cleanup outcomes become durable.
- v11 creates one Pane ResourceScope for each existing Pane and moves that
  Pane's requested policy into it. A logical Session parent does not claim
  hard enforcement.

### `principals`

- Stable opaque `id`.
- Optional `parent_id`, monotonic `epoch`, `created_at_ms`, and optional
  `revoked_at_ms`.
- Principals are tombstoned. They are not hard-deleted while descendants or
  durable audit references may exist.
- The migration creates or maps a local root Principal. Existing logical
  owner strings remain non-security audit/fencing data and do not become
  authenticated identities.

### Authority Records

- `attachment_authorities` is keyed by `pane_id`; the Session is derived
  through the Pane.
- Reclaim token hashes, owner, generation, deadline, and attachment state move
  with the existing authority record.
- `leases` remains keyed by `session_id`.
- `idempotency_records` remains global and unchanged except for migration-safe
  schema compatibility.

## 5. Migration Procedure

1. Open the database in WAL mode with foreign keys enabled.
2. Reject schemas newer than v11 and require a verified pre-migration backup.
3. Start one immediate transaction and validate v10 invariants.
4. Create the local root Principal and the new table set.
5. Copy every Session, Pane, Task, Lease, attachment authority, idempotency
   record, and history field without changing public identities.
6. Create one `pane_workers` row for each existing Session worker identity and
   associate it with that Session's sole Pane.
7. Attach each existing Task and attachment authority to that same Pane.
8. Create one Pane ResourceScope per Pane and copy requested resource policy.
9. Drop obsolete tables and indexes only after copy validation succeeds.
10. Run foreign-key, uniqueness, row-count, policy, and token-hash checks.
11. Set `PRAGMA user_version = 11` and commit.

Any validation or copy failure rolls back the entire migration. There is no
automatic downgrade; the pre-migration backup is the recovery path.

## 6. Compatibility Rules

- Existing Session, Pane, Task, Lease, and attachment identities are stable.
- V0 databases with exactly one Pane and one Task migrate without manual
  repair.
- Anomalous v10 databases with duplicate workers or orphaned references fail
  closed and report the invariant violation.
- A v10 binary must refuse a v11 database rather than reading partial state.
- Migration preserves attachment authority, idempotency replay data, terminal
  history, and the requested Pane policy.
- Terminal bytes, PTYs, live worker PIDs, and effective runtime metrics are not
  reconstructed from SQLite.

## 7. Acceptance Cases

Future implementation uses these stable test IDs:

| ID | Case | Expected result |
|---|---|---|
| SHARED-003 | Migrate a populated v10 database to v11 | Sessions, Panes, Tasks, Leases, attachment authority, idempotency records, policy, and history survive; old identities remain stable |
| SHARED-004 | Start the multi-Pane topology from migrated state | qwed discovers, adopts, reports health for, and routes commands to workers by Pane; one Pane loss does not mark unrelated Panes lost |
| SHARED-005 | Exercise interactive and observer channels on one Pane | Interactive authority is exclusive for that Pane; the observer receives snapshot/output only and cannot input, resize, or control |
| SHARED-006 | Tear down a Pane ResourceScope containing detached jobs | The complete scope closes, no process or controller state remains, and the cleanup tombstone records the outcome |

Migration tests must cover a production-generated v10 database, concurrent
writers at shutdown, interrupted migration rollback, history retention,
attachment-token preservation, and a database containing the maximum retained
history.

## 8. Non-Goals

- Pane geometry and compositor persistence.
- `exec` versus `shell_exec` semantics or command-handle storage.
- Capability-token encoding, AuthContext internals, or revocation severity.
- Sandbox, namespace, UID, filesystem, or network isolation.
- Terminal replay persistence or disk-backed scrollback.
