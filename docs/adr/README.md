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
| [0013](0013-core-object-model.md) | V0 core object model and lifecycle | Accepted; amended by 0021 |
| [0014](0014-attachment-ownership-authorization.md) | Attachment ownership and authorization | Accepted; amended by 0021 |
| [0015](0015-worker-liveness-reconciliation.md) | Worker liveness and reconciliation | Accepted |
| [0016](0016-stage-3-retention-and-clock-policy.md) | Stage 3 Lease clock, TTL, and retention | Accepted |
| [0017](0017-stage-4-service-security-and-capabilities.md) | Stage 4 service, secret, resource, and teardown contracts | Accepted |
| [0018](0018-pane-resource-enforcement.md) | Pane resource enforcement tiers and dynamic limits | Accepted; amended by 0021 |
| [0019](0019-temporary-macos-local-signing.md) | Temporary local self-signed macOS distribution | Accepted temporarily |
| [0020](0020-linux-first-v0-release.md) | Linux-first V0 release and deferred public macOS | Accepted |
| [0021](0021-shared-prerequisites.md) | Shared prerequisites for Session layout and agent execution | Accepted |
| [0022](0022-session-layout-compositor.md) | Pane-targeted Attach and client compositor | Accepted; slice 1 implemented |
| [0023](0023-durable-pane-layout.md) | Durable Pane ordering, weight, z-index, and focus | Accepted; implemented |
| [0024](0024-pane-composition-controls.md) | Relative split, close, focus, and reorder controls | Accepted; implemented |
| [0025](0025-copy-and-quick-menu.md) | Pane-scoped visible-screen copy and client quick menu | Accepted; implemented |
| [0026](0026-fixed-floating-panes.md) | Fixed floating Panes with persisted z-order | Accepted; fixed phase implemented |
| [0027](0027-floating-bounds-and-keyboard-move.md) | Durable floating bounds and keyboard move/resize | Accepted; implemented |
| [0028](0028-mouse-drag-and-hit-testing.md) | SGR mouse drag, z-order hit-testing, and content routing | Accepted; implemented |
| [0029](0029-mouse-resize-handle.md) | Floating bottom-right mouse resize handle | Accepted; implemented |
| [0030](0030-floating-z-order.md) | Floating click-to-raise and persisted z-order | Accepted; implemented |
| [0031](0031-scrollback-and-mouse-copy.md) | Client scrollback navigation and focused-Pane mouse copy | Accepted; implemented |
| [0032](0032-agent-execution-model.md) | Fresh-process agent execution, command handles, events, and capability model | Accepted; first delivery implemented |
| [0033](0033-bubblewrap-sandbox-adapter.md) | External Bubblewrap sandbox adapter and fail-closed profile model | Accepted; Linux implementation complete |
| [0034](0034-sandboxed-pane-and-pty.md) | Sandboxed Pane, interactive PTY, and workspace path trust | Accepted; Linux x86_64 implementation complete |
