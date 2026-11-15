# Storage Migration: Pane Layout, Agent Security, and Sandbox Profiles

Status: Implemented through sandboxed Pane creation; schema v18 is current
Date: 2026-11-16
Authority: [ADR 0021](adr/0021-shared-prerequisites.md) and the v11 schema in
`crates/qwe-storage`; Pane layout metadata is governed by
[ADR 0023](adr/0023-durable-pane-layout.md); command metadata is governed by
[ADR 0032](adr/0032-agent-execution-model.md).

## 1. Purpose

Schema v11 removes the V0 one-Pane-per-Session and one-Task-per-Session
assumptions while preserving every valid v10 record. It introduces durable
Pane-worker, ResourceScope, and Principal records that the accepted shared
prerequisites require.

The implementation follows these invariants and compatibility rules. The
physical schema lives in `crates/qwe-storage`; this document remains the
migration contract rather than a second schema copy.

## 2. Pre-v11 Baseline Facts

- `sessions` stores the optional worker PID, PGID, and start token.
- `panes.session_id` is unique, so a Session cannot own multiple Panes.
- `tasks(session_id)` has a unique index, so a Session cannot own multiple
  Tasks.
- Pane resource policy is stored as columns on `panes`.
- Lease state is Session-scoped.
- Attachment authority is keyed by `session_id`.
- Idempotency records and task history are durable and must survive migration.
- The pre-migration schema used `PRAGMA user_version = 10`.

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

The following names describe the implemented logical records. Physical column
order and indexes are defined by `crates/qwe-storage`.

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
- v15 adds `high_trust`, revocation severity/deadline, and security-hold state.
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

## 5. Implemented Migration Procedure

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
| LAYOUT-006 | Restart qwed after changing focus in a multi-Pane Session | Pane IDs, layout order, weights, and focused Pane survive unchanged |

Migration tests must cover a production-generated v10 database, concurrent
writers at shutdown, interrupted migration rollback, history retention,
attachment-token preservation, and a database containing the maximum retained
history.

## 8. Non-Goals

- Freeform Pane geometry, split trees, and floating-overlay persistence.
- `exec` versus `shell_exec` semantics or command-handle storage in the v11
  migration; Stage 9.1 adds metadata-only command storage in v14.
- Raw capability-token storage. v15 stores token hashes and epoch snapshots;
  raw capabilities remain in mode-600 capability files or response memory.
- Sandbox, namespace, UID, filesystem, or network isolation.
- Terminal replay persistence or disk-backed scrollback.

## 9. v11 to v12: Durable Pane Layout

Schema v12 preserves every v11 identity and adds:

- `panes.kind` with `normal` as the migration default;
- `panes.layout_order`, unique within active Session layout membership;
- `panes.layout_weight`, default `1`;
- `panes.z_index`, default `0`;
- `session_layouts.focused_pane_id`.

The migration orders existing Panes by stable Pane ID, gives every Pane equal
weight, and selects the first ordered Pane as focus. The pre-migration backup
and all validation remain transactional. Focus is a user preference, not
attachment authority; `PANE_FOCUS` enforces the target Pane's attachment
authority separately.

The physical index on `(session_id, layout_order)` is non-unique because closed
Panes remain history and retain tombstone orders. Active layout membership is
limited to Pane workers in `active` or `exited` state; qwed and storage enforce
unique, complete ordering for that live set transactionally.

Floating Panes retain the same Pane identity and worker records, persist
`kind=floating` plus increasing `z_index`, and are excluded from tiled reorder
membership.

## 10. v12 to v13: Durable Floating Bounds

Schema v13 adds nullable `floating_x`, `floating_y`, `floating_rows`, and
`floating_columns` to `panes`.

- All four values are null or all are present.
- Only floating Panes may store nonzero bounds.

## 11. v13 to v14: Agent Command Metadata

Schema v14 adds `agent_commands` without changing Session, Pane, Task, Lease,
attachment authority, layout, or idempotency identities.

Each row stores:

- stable `AgentCommandId`, Session, Pane, and logical owner;
- worker process identity;
- optional process identity, which is null only while start is pending;
- command state and output mode;
- creation/update timestamps; and
- optional exit code or signal.

Command rows cascade with their Session or Pane history. `process_pid`,
`process_pgid`, and `process_start_token` are null together or populated
together. Program arguments, environment values, credentials, output bytes, and
event replay are not stored in this table.

The migration is transactional and sets `PRAGMA user_version = 14`. A v13
binary must refuse a v14 database. Existing databases are backed up before the
upgrade using the current migration procedure.

## 12. v14 to v15: Capability and Revocation State

Schema v15 adds:

- `principals.high_trust`;
- `principals.revocation_severity`;
- `principals.revocation_deadline_ms`;
- `principals.revocation_hold`;
- `agent_commands.principal_id`;
- `agent_commands.creator_capability_id`;
- `agent_capabilities`, containing capability ID, token hash, Principal,
  complete ancestor epoch snapshot, operation, scope, timestamps, expiry, and
  revocation time.

`local-root` migrates as high-trust. Existing agent command rows are attributed
to `local-root` with no creator capability because they predate enforcement.
New commands retain the exact creating Principal and capability.

Capability scope columns are mutually exclusive by kind: global, Session, Pane,
or command. A command scope includes both Pane and command identity. Raw tokens
and Principal epoch chains are never placed in idempotency responses.

The migration is transactional and sets `PRAGMA user_version = 15`. A v14
binary must refuse a v15 database.

## 13. v15 to v16: Capability Sandbox Profile

Schema v16 adds nullable `agent_capabilities.sandbox_profile_json`.

- `NULL` preserves the explicit unsandboxed capability path.
- A present value stores the complete versioned `SandboxProfile`.
- Existing v15 capabilities migrate with `NULL`; they never gain sandboxing
  implicitly.
- The pre-migration backup is named `.pre-v16-*`.

The migration is transactional and sets `PRAGMA user_version = 16`. A v15
binary must refuse a v16 database.

## 14. v16 to v17: Sandbox Audit Metadata

Schema v17 adds:

- `agent_capabilities.sandbox_audit_json`, containing the sandbox profile
  SHA-256 hash, effective seccomp cBPF SHA-256 hash, resolved Bubblewrap
  path/version, optional Landlock ABI, and probe timestamp;
- `agent_commands.sandbox_risk_json`, containing network-inherit, writable
  workspace, and credential-exposure flags.

Existing rows migrate with null audit metadata. The migration is transactional
and sets `PRAGMA user_version = 17`. A v16 binary must refuse a v17 database.

## 15. v17 to v18: Pane Sandbox Metadata

Schema v18 adds `pane_sandboxes`, keyed by Pane ID:

- `profile_json` stores the exact capability-enforced v1 profile;
- `audit_json` stores the backend, profile hash, seccomp hash, optional
  Landlock ABI, and probe timestamp;
- `capability_id` links the Pane to the `sandboxed_pane` capability that
  authorized creation; and
- `created_at_ms` records when creation committed.

The row is inserted in the same transaction as the Session/Pane/Task or
Pane/worker/Task records. A missing sandbox row therefore means an unsandboxed
Pane, never an unknown downgrade. Deleting a Pane cascades its sandbox metadata.
The migration is transactional and sets `PRAGMA user_version = 18`. A v17
binary must refuse a v18 database.

- Bounds are clamped to the current usable viewport at render time.
- Migration leaves every existing floating Pane with null bounds, preserving
  the centered popup default.

`PANE_FLOATING_UPDATE` persists a complete rectangle under LayoutMutation
authority and preserves Pane, worker, Task, and ResourceScope identity.
