# ADR 0001: Rust for the QWE Runtime

Status: Accepted
Date: 2026-11-03

## Decision

Use Rust for QWE production code.

## Rationale

The POC found all three candidates technically viable. Rust was selected for
its combination of explicit ownership, typed error handling, narrow system
dependencies, and healthy PTY/SQLite/VT ecosystem. It carried no runtime
hard-gate penalty.

## Consequences

- The workspace uses one Rust toolchain and Cargo-based dependency management.
- Unsafe code is limited to platform adapters and documented in a `SAFETY`
  comment.
- Go and C++ POC code remain reference evidence and are not production inputs.
