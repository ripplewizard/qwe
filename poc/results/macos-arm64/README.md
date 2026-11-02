# macOS ARM64 Results

Secondary comparison and preflight result directory. These results exercise
platform behavior but do not replace the Linux x86_64 primary target.

Current full-suite evidence revision is `9abaff7-dirty`; final scoring requires
regeneration from a clean commit.

Current coverage:

- T01/T02 and T05–T07/T09/T10 pass for Rust, Go, and C++.
- T03/T04/T08/T11 have automated preflights and platform manual notes, but
  remain `blocked` pending independent human sign-off.
- T12 records the same terminal-loss boundary as Linux.
- P01–P12 and D01–D08 pass.
- RM01–RM05 pass; GPU-capable metrics report `unsupported`.
- DX01–DX03 pass.
- XP01/EX01/MC01 are generated for every language.

The PTY resize implementation sends an explicit SIGWINCH after `TIOCSWINSZ`.
Manual observations are under `manual/`.
