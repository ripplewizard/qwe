# Stage 6.4 Shared-Prerequisites Spike

This directory is a disposable experiment for
[ADR 0021](../../docs/adr/0021-shared-prerequisites.md). It does not add
production topology or protocol code.

The spike uses the unmodified production `qwed` and `qwe-worker` binaries:

1. `qwed` creates and closes a real V0 Session so the migration starts from a
   production-generated schema-v10 database.
2. A spike-local migration creates the proposed Principal, ResourceScope,
   multi-Pane, Pane-worker, and channel-grant records while preserving the V0
   Session, Task, Pane, resource policy, lease, and attachment state.
3. A spike-local supervisor starts 2-4 unmodified workers and groups their
   distinct worker identities under one logical Session.
4. A second supervisor process discovers and adopts the workers, checks health,
   verifies isolated terminal screens and journals, reads each Pane's real
   cgroup v2 policy and usage, targets one Pane through Attach, exercises a
   read-only observer projection, forces a task-limit event, and verifies one
   worker failure does not affect the other Panes.
5. A final process kills every worker, removes runtime sockets, and verifies
   there are no residual cgroup scopes.

Production V0 rejects `read_only` Attach mode. The spike therefore places the
observer behind a qwed-owned bridge: the bridge holds the worker channel while
the observer receives only snapshot/output and cannot send `INPUT`. This is
evidence for the proposed `ChannelProfile` boundary, not a production
implementation.

Run on Fedora Linux with a cgroup v2 systemd user manager:

```sh
just stage6-shared-prerequisites-spike
```

The command writes all runtime state and evidence under
`target/stage6.4/<timestamp>-<id>/`. It intentionally leaves that directory in
place when a check fails so the failure can be inspected. Run artifacts are
ignored by Git.
