# ADR 0003: Versioned Local IPC

Status: Accepted
Date: 2026-11-03

## Decision

Use length-prefixed JSON frames over owner-only Unix Domain Sockets. Every
request carries a protocol version, request ID, operation, and bounded payload.
Mutating operations also carry an idempotency key.

## Security

- Reject frames above the configured limit.
- Verify peer credentials and require the same OS user.
- Set socket mode to `0600`.
- Never expose the socket over TCP.

## Consequences

Mismatched versions and authentication failures are explicit protocol errors.
The POC tab-delimited protocol is not a production compatibility promise.
