# Experiment 0042: Stage 7 and 8 Closure

Date: 2026-11-14
Status: Pass

## Scope

This experiment closes the remaining Stage 7 and Stage 8 evidence gaps and
records acceptance of the next Agent Execution contract.

## Installed-Systemd and Long-Run Qualification

Fedora, Ubuntu 22.04, Ubuntu 24.04, and Debian 12 pass the R5
installed-systemd resource matrix. Two consecutive six-hour runs on Ubuntu
22.04 exceed the five-hour Stage 7 long-run requirement. Both runs used 16
Sessions by two Panes, completed 11 qwed restarts, retained bounded Attach
latency, and ended with no residual worker, socket, or Pane scope.

Detailed long-run measurements and defect history are in
[Experiment 0041](0041-linux-vm-qualification.md). Raw databases, progress,
results, and logs remain under `/home/Public/qwe-vms/evidence/`.

## Observer Closure

`SHARED-005` now passes on one Pane with a live interactive writer:

- the observer receives a complete snapshot;
- subsequent interactive output is mirrored to the observer;
- observer input is rejected with `read_only`;
- the interactive writer remains the only write authority;
- qwed restart recovery recreates both channels using the accepted reclaim
  boundary;
- observer and worker teardown leave no residual state.

The qwed-owned observer is implemented without adding a second worker Attach
mode or a second Pane/Task state path.

## Verification

- `just stage7-multi-pane`: pass, including SHARED-005.
- `just attach-e2e`: pass, including public observer acceptance and write
  rejection.
- `just stage7-check`: pass.
- `just stage8-check`: pass.
- Protocol schema: 12 frame types and 143 stable test IDs pass.

Stage 8 LAYOUT-001 through LAYOUT-017 pass on macOS, Fedora, rootless Podman,
and the three installed-systemd VM targets.

## Next Decision

[ADR 0032](../adr/0032-agent-execution-model.md) is accepted. It selects
fresh-process `exec`, command handles with `wait_idle`, bounded command events
and replay, read-only observation, capability and revocation enforcement,
per-command credentials, and explicit recovery states.
