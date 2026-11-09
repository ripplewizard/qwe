# Experiment 0027: Linux Container Compatibility

Status: Complete for the container smoke scope
Date: 2026-11-06
Host: Fedora Linux 35 x86_64 (`istoria`)
Runtime: Podman 3.4.7, rootless, cgroup v2

## Purpose

Verify that the Fedora-built QWE 0.1.1 executables can start and perform core
Session operations in other glibc-based Linux distributions. This experiment
does not qualify those distributions for release support or exercise the
installed-systemd/resource-control path.

## Image Matrix

Docker Hub was not reachable directly from the test network. Images were
pulled from `docker.1panel.live` and tested with container networking disabled.

| Distribution | Image ID | Repo digest | glibc |
|---|---|---|---|
| Ubuntu 22.04 | `a7b0f6af3254` | `sha256:076e7bf82319619f28dd0aa9c02ab34b055ad37100395b4f49a2ffba04fb5e11` | 2.35 |
| Ubuntu 24.04 | `d9b8c34d61a9` | `sha256:78d4744ead1f5b19b04d0df558ab182a5300903900755e515b2ca4d10eb2044d` | 2.39 |
| Debian 12 | `214b2bda8a5c` | `sha256:268fb24c39bca92d326dd83ea3abebe4a3f7751c804ffef23bb6d857dd343ff8` | 2.36 |

## Procedure

`tests/fixtures/container_smoke.sh` runs inside each container with only the
release binaries and test script mounted read-only. For every round it:

1. Starts `qwed` with an isolated SQLite store and Unix socket.
2. Creates a shell Session and waits for its output marker.
3. Sends input and executes the shell function `qwe_echo`.
4. Verifies the rendered terminal snapshot.
5. Reads Task history.
6. Sends `SIGKILL` to `qwed`, restarts it with the same store, and verifies the
   same Session/PID is adopted.
7. Runs `kill-server --confirm` and verifies clean teardown.

Run:

```sh
just container-smoke \
  "docker.1panel.live/library/ubuntu:22.04 docker.1panel.live/library/ubuntu:24.04 docker.1panel.live/library/debian:12"
```

## Results

`CONTAINER-001` passed three consecutive rounds on every image:

| Distribution | Session create | Input | Shell function | Snapshot | Task history | Restart adoption | Kill server |
|---|---|---|---|---|---|---|---|
| Ubuntu 22.04 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| Ubuntu 24.04 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| Debian 12 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |

The host Fedora run used the same script as a control and passed.

## Capability Boundary

- The containers do not include a systemd user manager. Hard cgroup resource
  limits, service installation, automatic service restart, and installed
  package behavior were not exercised.
- The test reused Fedora-built glibc binaries; it does not establish support
  for older glibc, musl, ARM64, or every Debian/Ubuntu release.
- Passing this smoke does not add Ubuntu or Debian to the supported-platform
  matrix. Full release, service, resource, migration, and manual gates remain
  required before promotion.
- Ubuntu 20.04 and Debian 11 were not included because their glibc versions
  predate the Fedora 35 build baseline.
