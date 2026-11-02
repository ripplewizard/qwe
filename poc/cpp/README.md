# C++ POC

The C++ implementation will prioritize libvterm, libuv, POSIX PTY APIs, and
optionally Boost.Asio. These candidates are not yet dependencies.

Current terminal-state POC uses system libvterm; SQLite persistence uses the
system SQLite C API.
