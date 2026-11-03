# ADR 0008: Concurrent Attach and Latency Budget

Status: Accepted
Date: 2026-11-03

## Decision

`qwed`, `qwe-host`, and session workers serve client connections concurrently.
Host and worker shared state is protected only while constructing a response;
large bounded-snapshot writes happen outside that state lock.

V0 uses a real 8-client benchmark with 64 Attach requests while the PTY emits
continuous output. The release gate is zero failed requests, P95 no greater
than 3,000 ms, and P99 no greater than 5,000 ms.

## Consequences

A slow Attach client cannot serialize all control requests. Future TUI and
observer work must preserve this connection-level concurrency.
