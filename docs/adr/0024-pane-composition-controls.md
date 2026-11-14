# ADR 0024: Pane Composition Controls

Status: Accepted
Date: 2026-11-10
Extends: [ADR 0023](0023-durable-pane-layout.md)
Implementation state: Relative split, close, focus, and reorder controls are
implemented in the API and client compositor.

## Context

Durable layout metadata is not sufficient if Panes can only be appended and
ordered outside the compositor. The client needs to create, remove, and reorder
Panes without bypassing qwed authority or corrupting the unique live layout
membership.

Closed Panes remain durable history. Treating them as active layout members
makes close semantics ambiguous, blocks compact reorder operations, and causes
restart-recovered layouts to show resources that no longer exist.

## Decision

### 1. Relative split

`PANE_CREATE` accepts optional `after_pane_id`. qwed verifies the source Pane
belongs to the Session, creates the worker, and inserts the new Pane immediately
after the source in live layout order. Existing live Pane order is shifted
atomically. Omitting the field appends.

### 2. Close

Existing `PANE_CLOSE` remains the close operation. The client focuses the
target Pane first, then closes it. Closed or lost worker records remain
available as history but are excluded from active composition and reorder
membership.

The compositor refuses to close the last visible Pane. The Session itself must
be closed instead.

### 3. Reorder

`PANE_REORDER` accepts the complete ordered list of live Panes in one Session.
qwed verifies the list exactly covers that membership, authorizes a Session
layout mutation, and stores the result transactionally. Storage uses a
temporary negative-order phase so live uniqueness can change without transient
collisions.

The database index over `(session_id, layout_order)` is non-unique because
historical Panes may retain tombstone orders. Exact live-membership uniqueness
is enforced by qwed and storage methods.

### 4. Compositor shortcuts

- `Ctrl-A c`: split after focus using a new `sh` Pane.
- `Ctrl-A x`: close the focused Pane.
- `Ctrl-A h`: move focus left in layout order.
- `Ctrl-A l`: move focus right in layout order.
- Existing `Ctrl-A n/p`, numeric focus, and detach shortcuts remain.

Focus changes are persisted through `PANE_FOCUS`. Structural actions emit one
new layout, resize all surviving Panes, and update only client projection state.

## Non-Goals

- Arbitrary split ratios or mouse dragging.
- Multiple Panes behind one worker.
- Closing the final Pane without closing its Session.
- Lost-Pane visual diagnostics and persistent semantic history.

## Validation

LAYOUT-007 covers relative split, attach, input, and close. LAYOUT-008 covers
focus-preserving reorder. Both run through real qwed, workers, PTYs, and the
interactive compositor in `just stage8-layout`.

## References

- [ADR 0022](0022-session-layout-compositor.md)
- [ADR 0023](0023-durable-pane-layout.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
