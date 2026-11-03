# Dependency Review

Status: complete for direct POC dependencies.

## Rust

- `libc 0.2.191`: platform system calls and process/signal support.
- `portable-pty 0.9.0`: PTY allocation and child spawning.
- `rusqlite 0.41.1`: SQLite access with bundled SQLite.
- `vt100 0.18.1`: VT parsing and screen state.

The direct dependency set is small, actively used, and has clear PTY, VT,
SQLite, and platform ownership boundaries.

## Go

- `github.com/creack/pty v1.1.24`: PTY allocation.
- `github.com/mattn/go-sqlite3 v1.14.52`: SQLite via cgo.
- `github.com/hinshun/vt10x v0.0.0-20220301184237-5011da428d02`: VT parser.

The `vt10x` dependency is a dated pseudo-version and is the main maintenance
risk. The SQLite package also requires a native compiler toolchain through
cgo.

## C++

- SQLite3 supplied by the platform.
- `libvterm` supplied through pkg-config.
- C++20 standard library for threads, sockets, and process ownership.

The dependency surface is capable but requires the most manual adaptation for
VT callbacks, PTY handling, linking, and cross-platform build setup.

## Maintenance Risks

- Go should replace or own `vt10x` before production adoption.
- C++ requires explicit ownership and platform integration discipline.
- Rust has the strongest current direct-dependency health signal.
- Transitive vulnerability and license review remains a separate production
  task; this report is not a legal audit.
