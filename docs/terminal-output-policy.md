# Terminal Output Policy

Date: 2026-11-15
Status: Design accepted by ADR 0034; implementation pending

## Scope

This policy governs terminal output from untrusted commands, including
sandboxed Panes and Agent commands projected to a human terminal. It prevents a
program's output from becoming a clipboard write, terminal query response, or
new PTY input.

Filtering runs in the worker/qwed output path before journal retention and
before bytes reach any Attach client. Client-side filtering is insufficient
because every client could implement a different policy.

## Policy Modes

- `restricted`: default for Agent and sandboxed-Pane output. Only display state
  sequences are retained and replayed.
- `full`: trusted human Pane compatibility mode. It is explicit policy and does
  not apply automatically to sandboxed output.

Both modes retain the existing control-frame ordering with terminal output.

## Restricted Allowlist

The initial whitelist is intentionally narrow:

- SGR text attributes and colors;
- cursor movement and positioning;
- erase, insert, delete, scroll, and save/restore screen;
- alternate-screen enter/exit;
- character-set selection needed by terminal applications;
- bracketed paste, focus, and mouse mode controls that cannot write input;
- standard BEL.

The implementation parses streamed escape sequences, not individual chunks.
Sequences split across output reads are classified once.

## Restricted Denylist

These sequences are dropped in `restricted` mode:

- OSC 52 clipboard read/write;
- OSC 0/1/2 title changes;
- OSC 8 hyperlinks;
- desktop notifications and taskbar progress;
- OSC 4/10/11/12 color mutation and color queries;
- CSI 6n cursor-position report;
- CSI 21t title report;
- DECRQSS and other terminal capability/status queries;
- device attributes, terminal identification, and equivalent responses;
- any unknown or malformed escape sequence.

Dropping a query prevents the receiving terminal from generating a reply that
could be delivered back to the PTY as keyboard input. If a future policy permits
a query, its response types and byte length must be explicitly bounded.

## VT State and Journal

Filtering happens before:

- command event retention;
- Attach output offsets;
- terminal-state replay;
- logging; and
- human rendering.

For restricted streams, the journal and VT state consume filtered bytes so a
replay cannot reintroduce a dropped sequence.

## Input Audit Interaction

- Input audit defaults to target, Principal, length, timestamp, profile version,
  and effective Landlock level.
- Raw input is stored only with explicit policy opt-in.
- When terminal ECHO is disabled, raw input and any hash are omitted even when
  content audit is enabled.
- Input and resize events share the same ordered stream as output.

## Configuration Rules

- Enabling a denied OSC sequence requires a named policy change, not an
  undocumented environment variable.
- A broader policy on an unsandboxed trusted Pane remains separate from Agent or
  sandboxed-Pane policy.
- Unknown policy names and unknown sequence categories fail closed.

## Verification

- unit vectors for every allowed and denied sequence, including split chunks;
- title-set followed by title-report cannot create PTY input;
- OSC 52 cannot write the host clipboard;
- alternate-screen TUI rendering remains functional in restricted mode;
- journal replay contains no dropped sequence;
- observer and reconnect clients receive identical filtered bytes;
- `full` mode remains opt-in for non-sandboxed trusted Panes.
