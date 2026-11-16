# Experiment 0040: Stage 8 Cross-Platform Smoke

Date: 2026-11-11
Status: Pass on macOS ARM64, Linux x86_64, and rootless Podman

## Environments

- macOS 26.5.1 arm64 (`ulysses`), native Rust 1.99.1.
- Fedora Linux 35 x86_64, local development host.
- Rootless Podman on Linux x86_64 with `python:3.12-slim` and project release
  binaries.

## Results

- macOS `just stage8-check`: pass, including LAYOUT-001 through LAYOUT-017.
- macOS `just attach-e2e`: pass.
- Linux `just stage8-check`: pass.
- Linux `just stage7-multi-pane`: pass.
- Linux `just attach-e2e`: pass.
- Podman `python:3.12-slim` Stage 8 layout probe: pass, all LAYOUT rows.
- Podman `just container-smoke`: pass on Ubuntu 22.04, Ubuntu 24.04, and
  Debian 12.

## Portability Fixes

- Linux-only cgroup parsing helpers are now compiled out of non-Linux
  non-test builds.
- Linux-only resource counters are gated from the macOS worker build.
- Clipboard provider selection is initialized idiomatically per target OS.
- The macOS PTY probe drains compositor output before waiting for exit,
  preventing a test-harness output-buffer deadlock.

## Limitations

Podman is not installed-systemd, cgroup v2 delegation, OOM, or resource
qualification. Stage 7 multi-Pane scope cleanup remains Linux-host evidence and
cannot run on macOS because its fixture reads `/proc`.
