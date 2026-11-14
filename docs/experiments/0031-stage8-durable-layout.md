# Experiment 0031: Stage 8 Durable Layout

Date: 2026-11-10
Status: Pass on Linux x86_64
Related: [ADR 0023](../adr/0023-durable-pane-layout.md)

## Scope

Validate schema v12 migration and restart-stable Pane ordering, weight,
z-index, and focus without changing worker or Pane identity.

## Observations

- A populated v10 database migrated transactionally through v11 to v12 with a
  verified pre-v12 backup.
- A v11 database containing two Panes migrated to unique sequential layout
  order and equal weight while preserving both Pane IDs.
- `PANE_FOCUS` persisted the selected Pane under attachment authority.
- `just stage8-layout` passed LAYOUT-001 through LAYOUT-006, including
  compositor focus switching and qwed restart.
- Default Attach now resolves persisted focus before layout order.

## Limitations

- Viewport rectangles remain derived; freeform geometry and split trees are
  not persisted.
- Split, close, and reorder controls remain future Stage 8 work.
- Floating Pane kind is reserved in storage but has no compositor behavior.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
