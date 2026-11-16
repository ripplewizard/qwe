# Experiment 0033: Stage 8 Copy and Quick Menu

Date: 2026-11-11
Status: Pass on Linux x86_64
Related: [ADR 0025](../adr/0025-copy-and-quick-menu.md)

## Scope

Validate focused-Pane visible-screen copy, explicit clipboard success, and a
client-only quick menu through the real interactive compositor.

## Observations

- Copy mode selected the focused Pane's full visible screen.
- The selected text was written through an explicit test clipboard command and
  the compositor reported copied byte count plus provider.
- A missing or failing provider follows an explicit copy failure path.
- Menu open, close, and action keys were consumed by the client.
- A shell marker sent after menu close executed normally, with no leaked menu
  key prefixed to the command.
- `just stage8-layout` passes LAYOUT-001 through LAYOUT-010.

## Limitations

- Copy covers the visible screen only; scrollback and persistent selection
  generation are future work.
- Mouse selection and drag behavior are not implemented.
- Clipboard desktop integration is provider-based and not tested on all display
  servers.
- macOS and installed-systemd qualification for this slice remain future
  matrix work.
