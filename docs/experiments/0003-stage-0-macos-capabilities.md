# Experiment 0003: Stage 0 macOS Security and Resource Capabilities

Status: Passed investigation
Date: 2026-11-04

## Questions

1. Can a macOS process store and retrieve a secret through Keychain without
   placing the secret in argv, the environment, a workspace file, or logs?
2. Which macOS process controls can enforce Pane-level CPU, memory, and process
   count limits without claiming unsupported behavior?
3. Does `vt100` expose enough state for the accepted Attach snapshot contract?

## Environment

- macOS 26.5.1, build 25F80
- Mac15,7, arm64
- `vt100` 0.18.1
- Rust 1.99.1

## Commands

```sh
just stage0-macos-spike
just stage0-check
```

The macOS probe uses a temporary Keychain created in a private temporary
directory. It generates random in-memory key material and a secret with
`SecRandomCopyBytes`, exercises `SecKeychainAddGenericPassword` and
`SecKeychainFindGenericPassword`, and deletes the item and Keychain during
process cleanup. No secret is accepted through argv.

## Results

Observed probe output:

```json
{"keychain_create":0,"keychain_add":0,"keychain_find":0,"keychain_read_matches":true,"keychain_delete_item":0}
{"rlimit_cpu":{"exit_or_signal":152,"setrlimit_failed":false},"rlimit_address_space":"setrlimit_failed","rlimit_process_count":"enforced"}
```

Exit or signal 152 is 128 + `SIGXCPU` (24). `RLIMIT_CPU` therefore terminated
the child after its one-second per-process CPU limit.

| Capability | Observation | Product conclusion |
|---|---|---|
| Keychain generic-password storage | Create, add, read, compare, and delete returned `errSecSuccess` | A process-level Secret Provider path is feasible |
| `RLIMIT_CPU` | Enforced per process with `SIGXCPU` | Useful as a defensive per-process backstop, not aggregate Pane CPU enforcement |
| `RLIMIT_AS` memory limit | `setrlimit` failed on this macOS version | Cannot be exposed as a supported Pane memory limit |
| `RLIMIT_NPROC` process limit | Child `fork` was denied after a low per-UID limit | Enforces a UID-wide process count, not a Pane process-tree limit |
| `RLIMIT_DATA` / `RLIMIT_RSS` | Header distinguishes data break growth from advisory resident-set preference | Neither defines a hard process-tree memory cap suitable for Pane policy |
| `taskpolicy -m 64` | A process that touched 128 MiB still succeeded | Does not demonstrate a reliable Pane memory cap and is not claimed as one |

The Keychain item used here is in a temporary keychain. This does not prove the
login-keychain user-presence, code-signing, ACL, or locked-keychain behavior of
a release build; those belong to the Stage 4 credential implementation.

## VT Result

`vt100` 0.18.1 exposes cursor position, visibility and style, application
cursor/keypad modes, alternate screen, bracketed paste, mouse protocol mode and
encoding, wide-cell occupancy, and resize state. The Stage 0 tests pass for
those fields.

The confirmed limitation is width resize: rows are truncated or padded
without terminal-style reflow. This is acceptable for the first Attach
implementation only if TUI acceptance explicitly covers it; a mismatch in
Neovim or htop is a trigger to evaluate `alacritty_terminal` behind the
QWE-owned `TerminalState` interface.

## Decisions and Ownership

Secret Provider:

- Owner: QWE platform workstream.
- Decision date: 2026-11-13.
- Candidate design: qwed acts as credential broker, reads the selected Keychain
  item only in memory after an explicit user-unlock flow, and transfers it to
  the child through an inherited anonymous socket/pipe descriptor. The child
  consumes the descriptor directly; the secret is never written to argv,
  environment, workspace files, logs, or SQLite.
- Required follow-up: verify a signed build, ACL/access-group behavior,
  locked-keychain errors, descriptor close-on-exec rules, and zeroization.
- Fallback: report macOS credential injection as `unsupported`. Do not silently
  substitute environment variables or files.

Resource policy:

- Owner: QWE platform workstream.
- Decision date: 2026-11-27.
- V0 must not claim Pane memory or process-count enforcement on macOS based on
  the controls tested here.
- CPU limits may use `RLIMIT_CPU` only as a per-process backstop. Aggregate
  Pane CPU accounting still requires supervisor-side monitoring and explicit
  capability reporting.
- If a process-tree memory/process-count primitive is not proven by the
  decision date, macOS reports those capabilities as `unsupported`.

## Not Proven

- Login Keychain or data-protection-keychain behavior for a signed release app.
- Non-interactive access after restart or while the keychain is locked.
- Aggregate Pane CPU, memory, or process-tree enforcement.
- TUI visual compatibility for Neovim and htop; that remains a Stage 2
  acceptance task.
