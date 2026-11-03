# ADR 0007: Control-State Reconciliation

Status: Accepted
Date: 2026-11-03

## Decision

`qwed` stores Sessions, Tasks, process identity, exit status, and Session
leases in SQLite WAL mode. Schema changes are monotonic and migrate existing
databases transactionally.

On startup, `qwed` registers its fencing epoch, reads the Host inventory, and
reconciles it with SQLite before accepting clients:

- Live Host Sessions update persisted state and process identity.
- Persisted Sessions absent from the Host become `lost`.
- Running Tasks become `exited` or `lost` from Host status and inventory.
- Lease records survive restart with owner, generation, and deadline.

Lease operations require the expected owner and generation. Expired leases can
only be replaced through claim or takeover, which increments the generation.

## Consequences

Control-plane restart does not erase history or silently report lost Sessions
as running. SQLite remains limited to control metadata; terminal bytes are not
written per frame.
