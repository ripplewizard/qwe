# ADR 0022: Pane-Targeted Attach and Client Compositor

Status: Accepted
Date: 2026-11-10
Implementation state: Slice 1 implements Pane-targeted Attach,
`SESSION_LAYOUT`, deterministic normal-Pane tiling, and the `session compose`
client. Geometry persistence, copy, menu, and floating Panes remain open.

## Context

ADR 0021 fixes one worker per Pane, but the public Attach channel still selects
the Session's default Pane. The client can create multiple Pane workers without
being able to present or operate them as one Session. Stage 8 needs that bridge
without creating a second terminal-state authority or weakening Pane-exclusive
interactive ownership.

The first layout slice must be testable before deciding persistent split trees,
copy selection, menus, or floating overlays. Those features depend on correct
routing and rendering, not only on Pane inventory.

## Decision

### 1. Pane-targeted Attach

`ATTACH_OPEN` gains an optional `pane_id`. When present, qwed:

- verifies the Pane belongs to the requested Session;
- selects that Pane's worker and attachment authority;
- rejects a missing, lost, or unreachable Pane explicitly;
- forwards the selected Pane identity to the worker and client.

Omitting `pane_id` preserves the existing default-Pane behavior. Each Pane
keeps one exclusive interactive attachment. A client may hold one exclusive
channel per displayed Pane; this does not grant multiple writers to one Pane.

qwed remains the authority for routing and attachment authorization. Workers
continue to own their PTY, VT state, journal, and input ordering. The compositor
does not inspect worker sockets or attach directly to workers.

### 2. Layout query and tiling

`SESSION_LAYOUT` is a read-only control operation with `session_id`, `rows`, and
`columns`. It returns Pane identity, worker state, focus hint, and non-overlapping
terminal-cell rectangles. The first implementation uses a deterministic
near-square grid for normal Panes, with leftover cells distributed from left to
right and top to bottom. A layout is bounded to 64 Panes.

The tiling function lives in `qwe-core` so qwed and clients use one algorithm.
Geometry is derived from the current viewport in this slice; it is not durable.
Persistent split ratios require a later storage revision.

### 3. Client compositor

`qwe session compose <session-id>`:

- queries the layout;
- opens one Pane-targeted Attach channel per Pane;
- parses each live stream into a client-local VT screen;
- renders all screens into their layout rectangles;
- routes input only to the focused Pane.

`Ctrl-A n`, `Ctrl-A p`, and `Ctrl-A 1..9` change focus. `Ctrl-A ]`, `Ctrl-A q`,
and `Ctrl-]` detach all channels. Existing `session attach` remains the
single-default-Pane path.

The compositor is a projection. It never becomes the authority for Pane
inventory, attachment state, process lifecycle, or terminal output.

### 4. First-slice boundaries

This decision does not add:

- persistent split ratios, pane order, z-index, or focus state;
- pane create/close shortcuts inside the compositor;
- mouse input, pane-clipped copy, quick menus, or system clipboard writes;
- floating or draggable overlays;
- multiple interactive clients for one Pane;
- a multiplexed layout socket or a second control daemon.

Those features require their own implementation slices and acceptance cases.
They must reuse Pane-targeted channels and the shared layout contract.

## Consequences

The client now depends on a VT parser and must handle all displayed Panes'
output independently. This increases client CPU and memory in proportion to
visible Pane count, but keeps qwed responsive and preserves per-Pane isolation.

Terminal-device queries emitted by a child are parsed by the compositor rather
than automatically answered by the physical terminal. Device-query forwarding
or synthetic replies require a separate compatibility decision and must not be
claimed by this slice.

## Validation

`just stage8-layout` exercises LAYOUT-001 through LAYOUT-005 with real qwed,
Pane workers, Pane-targeted Attach sockets, concurrent attachments, focused
input, and an interactive PTY compositor.

## References

- [ADR 0012](0012-terminal-attach-data-plane.md)
- [ADR 0021](0021-shared-prerequisites.md)
- [Stage 8 Session Layout](../stage8-session-layout.md)
