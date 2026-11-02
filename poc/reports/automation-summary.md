# Automated Test Summary

Status: current automated suite passes on both target preflight hosts. This is
not a completed POC matrix or final language decision.

## Platforms

- macOS ARM64: preflight only.
- Fedora Linux 35 x86_64: target platform; final scoring still requires a clean
  source tree.

## Passing Cases

`T01`, `T02`, `T05`, `T06`, `T07`, `T10`, `P01`, `P02`, `P03`, `P05`, `P06`,
`P08`, `P09`, `P11`, `P12`, `D01`, `D02`, `D03`, `D04`, `D05`, `D07`, and
`D08` pass for Rust, Go, and C++ on both hosts.

Evidence is stored under `results/macos-arm64/` and
`results/linux-x86_64/`. Linux evidence was retrieved through
`just test-linux-remote`.

## Not Yet Automated

- T09 Session Lease.
- T12 terminal and session recovery after daemon restart.
- P07 task timeout and P10 restart policy.
- D06 schema/daemon upgrade compatibility.
- RM01-RM05 resource monitoring and GPU capability.
- DX01-DX03, XP01, EX01, and MC01 final comparative evaluations.

## Manual Work Remaining

- T03 Neovim interaction.
- T04 htop interaction.
- T08 detach/attach.
- T11 terminal job control.

## Known Gaps

- Automated short-task history detects exit but does not persist exit codes.
- Terminal parsing covers one deterministic ANSI/Unicode fixture, not every VT
  sequence or grapheme cluster.
- POC IPC is limited and not yet a production wire contract.
- Source uses `f44f72e-dirty`; results are provisional.
