# Architecture Decision Records

ADRs are immutable after acceptance. Superseding an ADR requires a new record
that links to the replaced decision.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-rust-runtime.md) | Rust for the runtime | Accepted |
| [0002](0002-durable-terminal-host.md) | Durable terminal host | Amended by 0006 |
| [0003](0003-versioned-local-ipc.md) | Versioned local IPC | Accepted |
| [0004](0004-process-identity.md) | Process identity | Accepted |
| [0005](0005-terminal-state.md) | Terminal state and bounded attach | Accepted |
| [0006](0006-session-worker-recovery.md) | Isolated recoverable session workers | Accepted |
| [0007](0007-control-state-reconciliation.md) | SQLite control-state reconciliation | Accepted |
| [0008](0008-concurrent-attach-latency.md) | Concurrent Attach latency budget | Accepted |
| [0009](0009-control-plane-authority.md) | Control-plane authority and runtime reconciliation | Accepted |
| [0010](0010-product-capability-scope.md) | API keys, Pane limits, and forced teardown scope | Accepted |
| [0011](0011-consolidate-control-plane.md) | Consolidate qwed and qwe-host | Accepted |
| [0012](0012-terminal-attach-data-plane.md) | Terminal Attach data plane and snapshot handoff | Accepted |
| [0013](0013-core-object-model.md) | V0 core object model and lifecycle | Accepted |
| [0014](0014-attachment-ownership-authorization.md) | Attachment ownership and authorization | Accepted |
| [0015](0015-worker-liveness-reconciliation.md) | Worker liveness and reconciliation | Accepted |
| [0016](0016-stage-3-retention-and-clock-policy.md) | Stage 3 Lease clock, TTL, and retention | Accepted |
| [0017](0017-stage-4-service-security-and-capabilities.md) | Stage 4 service, secret, resource, and teardown contracts | Accepted |
| [0018](0018-pane-resource-enforcement.md) | Pane resource enforcement tiers and dynamic limits | Accepted |
| [0019](0019-temporary-macos-local-signing.md) | Temporary local self-signed macOS distribution | Accepted temporarily |
| [0020](0020-linux-first-v0-release.md) | Linux-first V0 release and deferred public macOS | Accepted |
