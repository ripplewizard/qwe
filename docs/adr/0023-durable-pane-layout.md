# ADR 0023: Durable Pane Layout Metadata

Status: Accepted
Date: 2026-11-10
Extends: [ADR 0022](0022-session-layout-compositor.md)
Implementation state: Schema v12, Pane layout metadata, durable focus, and
`PANE_FOCUS` are implemented.

## Context

ADR 0022 introduced Pane-targeted Attach and a client compositor while leaving
layout order and focus client-local. That is insufficient after detach or qwed
restart: the same Session can render in a different order, and the default
Attach target can change implicitly.

Pane identity, worker ownership, and attachment authority are already durable.
Layout metadata must extend those records without becoming authorization or
terminal state.

## Decision

### 1. Pane layout fields

Each Pane persists:

- `kind`: `normal` or reserved `floating`;
- `layout_order`: unique within the Session;
- `layout_weight`: a positive relative share for tiled geometry;
- `z_index`: an unsigned compositor order.

Schema v12 creates these fields and initializes existing Panes from stable Pane
identity order. Existing Pane IDs do not change.

### 2. Durable focus

Each Session has a persisted `focused_pane_id`. Focus is a user preference, not
attachment authority and not a Lease fence. It may name an exited Pane while
that Pane remains visible.

`SESSION_LAYOUT` uses explicit `focused_pane_id` as a request hint, otherwise
the persisted focus, otherwise the first Pane. `PANE_FOCUS` is a mutating
operation that requires Active owner authority on the target Pane, or Held
owner plus generation when that Pane is Held. An unattached target may use a
current Session Lease owner and generation, matching Pane-creation authority.

The existing default-Pane Attach path resolves persisted focus first, then
layout order. This preserves the "default Pane" concept while making its
selection explicit and restart-stable.

### 3. Shared geometry

qwed and the compositor continue to use the `qwe-core` tiling function. It
sorts by `layout_order`, gives columns a width proportional to
`layout_weight`, and returns `kind`, `layout_order`, `layout_weight`, and
`z_index` with the rectangle.

Slice 2 does not add split/close/reorder controls. New Panes append at the next
layout order; all created Panes are `normal`, weight `1`, z-index `0`.

### 4. Migration

v11 to v12 is transactional and creates a verified pre-migration backup.
Existing Panes receive sequential layout order by stable Pane ID and equal
weight. Focus defaults to the first Pane. No Session, Pane, Task, worker,
Lease, attachment, idempotency, or ResourceScope identity changes.

## Consequences

Layout state survives qwed restart and can safely drive the default Attach
target. Clients still own rendering and shortcut interpretation. Adding
floating geometry or split controls later can update metadata through new
authorized operations without changing Pane/worker identity.

## Validation

- Schema migration from populated v10 and v11 databases.
- LAYOUT-006 verifies focus and layout order across qwed restart.
- `just stage8-layout` retains LAYOUT-001 through LAYOUT-005 coverage.

## References

- [ADR 0021](0021-shared-prerequisites.md)
- [ADR 0022](0022-session-layout-compositor.md)
- [Storage Migration](../storage-migration.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
