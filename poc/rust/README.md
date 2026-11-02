# Rust POC

The Rust implementation will prioritize `portable-pty` and
`alacritty_terminal`, with Tokio for asynchronous daemon work. These candidates
are not yet dependencies.

Current terminal-state POC uses `portable-pty` plus `vt100`; SQLite persistence
uses bundled `rusqlite`.

