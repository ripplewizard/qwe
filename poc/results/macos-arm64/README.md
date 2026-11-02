# macOS ARM64 Results

These are macOS preflight results. They verify behavior and expose platform
differences but do not provide official Linux comparison scores.

The PTY resize result requires explicit SIGWINCH delivery after `TIOCSWINSZ` on
macOS. This platform behavior is recorded in each T07 result's limitations.

