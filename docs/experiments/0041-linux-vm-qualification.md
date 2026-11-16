# Experiment 0041: Linux VM Qualification

Date: 2026-11-14
Status: Pass; installed-systemd matrix and two extended long runs complete

## Environment

- Host: `istoria`, Fedora 35 x86_64, KVM available.
- Hypervisor: rootless QEMU 6.1 with KVM, user networking, and qcow2 overlays.
- VM storage: `/home/Public/qwe-vms`.
- Guests: Ubuntu 22.04, Ubuntu 24.04, and Debian 12 amd64.
- Each guest: 2 vCPU, systemd, cgroup v2, user manager enabled. Ubuntu 22.04 was
  raised to 4 GiB after the first long soak exposed guest-level OOM pressure;
  the other guests use 2 GiB.

## Results

All three guests pass:

- Stage 8 LAYOUT-001 through LAYOUT-017.
- Stage 7 resource profile, policy, event retention, restart, and detached scope
  cleanup probe.
- Stage 7 multi-Pane inventory, qwed restart adoption, failure isolation, and
  complete scope cleanup.
- systemd service crash restart, worker survival/adoption, binary replacement,
  graceful stop, Session close, and Task stop.
- RG-004 memory, CPU, and task-count pressure while control health remains
  responsive.
- 30-second 8-Session x 2-Pane soak with repeated qwed restarts.

The first five-hour Ubuntu 22.04 attempt used a 2 GiB guest and failed after
about 2.5 hours. The 4 GiB retry reached the same restart point without a
kernel OOM, so guest memory pressure was only one contributing factor.

Both attempts failed while restarting qwed after the fifth 30-minute interval.
The soak fixture reported that qwed did not accept a connection within its
15-second startup deadline. Replaying the captured 4 GiB database with an empty
runtime directory reproduced startup taking 59.948 seconds. The database held
2,184 Panes, including 2,151 already-lost Panes whose Tasks were already
terminal. Startup repeated `mark_pane_lost`, which scans every Pane and worker
for the Session, for each already-lost churn Pane.

Startup reconciliation now skips that repeated storage scan when the worker is
already `lost` and its Task is `exited` or `lost`, while still reconciling a
lost worker that has a running or stopped Task. The same captured database now
starts in 0.411 seconds. A ten-minute 8-Session by 2-Pane accelerated run
completed nine qwed restarts before the fix. The fixed release build then
completed a six-minute 16-Session by 2-Pane run with five qwed restarts, Attach
P95 23.714 ms, P99 28.791 ms, and peak RSS 929,431,552 bytes. The new
`stage7-reconciliation-scale` regression seeds 2,000 already-lost Panes and
requires qwed to become ready within five seconds; the current run starts in
40.697 ms and preserves the live Session. The final five-hour run started at
`2026-11-11T09:32:33Z` as
`qwe-stage7-soak-20261111T093233Z.service`.

The `2026-11-13` five-hour attempt completed the full workload, but its final
`soak-churn` shutdown exceeded the fixture's ten-second control timeout. The
sixteen normal Sessions had already closed successfully; the churn Session had
accumulated 4,109 historical Panes. Peak aggregate RSS was 1,564,213,248 bytes
on a 4 GiB guest, and no OOM occurred. This isolated the failure to teardown
latency rather than resource exhaustion.

The new `stage7-teardown-scale` regression keeps two real active Panes and
seeds 4,000 completed historical Panes in the same Session before measuring
`SESSION_KILL`. On the Ubuntu 22.04 VM the pre-fix release took 7.785 seconds.
The fixed release takes 0.277 seconds. Replaying the captured five-hour
database reduced its 4,110-Pane teardown from 6.962 seconds to 0.430 seconds.
The regression also fixed startup handling for `closed/stopped` history, which
previously repeated lost-Pane reconciliation and exceeded the startup deadline.

Two consecutive six-hour runs then passed on Ubuntu 22.04 with the fixed
release:

- `qualification-20261113T145723Z`: 11 restarts, Attach P95 27.955 ms, P99
  38.372 ms, peak RSS 1,513,373,696 bytes.
- `qualification-20261114T005443Z`: 11 restarts, Attach P95 27.910 ms, P99
  37.728 ms, peak RSS 1,536,249,856 bytes.

Both runs completed the full workload and final teardown without a
`failure.json`, residual qwed/worker process, socket, or Pane scope. The
six-hour duration exceeds the five-hour Stage 7 requirement. Fedora, Ubuntu
22.04, Ubuntu 24.04, and Debian 12 continue to pass the shorter R5
installed-systemd matrix.

Failure evidence and databases are archived under
`/home/Public/qwe-vms/evidence/`; generated guest state remains outside the
repository.

## Configuration Finding

Ubuntu 22.04 initially delegated only `memory pids` to the user manager. Adding
`Delegate=cpu cpuset io memory pids` for `user@.service` exposed
`cpuset cpu io memory pids` and made CPU-scoped resource tests pass.

Debian's genericcloud image did not consume the NoCloud seed under the direct
QEMU boot used here. A fresh overlay was provisioned with `virt-customize` for
the `qwe` account, SSH key, sudo rule, SSH enablement, linger, and networking.

## Evidence

VM launch assets and logs live under `/home/Public/qwe-vms`. The host checked
out worktree remains unchanged by VM provisioning.
