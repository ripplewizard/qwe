# Linux x86_64 Results

Primary POC result directory. Distribution and release are environment
metadata, not target restrictions.

Run `just test-linux` on the Linux host. Current full-suite evidence revision is
`9abaff7-dirty`; final scoring requires regeneration from a clean commit.

Current coverage:

- T01/T02 and T05–T07/T09/T10 pass for Rust, Go, and C++.
- T03/T04/T08/T11 have automated preflights and platform manual notes, but
  remain `blocked` pending independent human sign-off.
- T12 records the current negative boundary: daemon restart does not recover
  terminal process/session/PTY/screen/input.
- P01–P12 pass for all implementations.
- D01–D08 pass, including isolated-filesystem ENOSPC.
- RM01–RM05 pass with explicit GPU capability reporting.
- DX01–DX03 pass, including the persisted alternate-screen TUI.
- XP01/EX01/MC01 are generated for every language.

Manual observations are under `manual/`. Structured per-case evidence is linked
from `poc/test-spec/matrix.md`.
