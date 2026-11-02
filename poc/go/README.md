# Go POC

The Go implementation will prioritize `creack/pty`, `os/exec`, `syscall`, and
goroutines/channels. These candidates are not yet dependencies.

Current terminal-state POC uses `creack/pty` plus `vt10x`; SQLite persistence
uses `mattn/go-sqlite3`.

