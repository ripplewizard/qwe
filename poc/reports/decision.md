# Language Decision Report

Status: in progress. Runtime hard gates have initial evidence; terminal manual
checks and failure-injection coverage remain.

## Hard Gates

| Gate | Rust | Go | C++ | Evidence |
|---|---|---|---|---|
| Basic terminal interaction | Pending T03/T04 | Pending T03/T04 | Pending T03/T04 | T05/T06 automated pass |
| Reliable process-group control | Pass | Pass | Pass | P02/P03 |
| Tasks survive daemon crash | Pass | Pass | Pass | P08 |
| Recoverable TaskInstance identity | Pass* | Pass* | Pass* | P09; macOS token is coarse |
| Acceptable dependency health | Not run | Not run | Not run | |
| Explicit IPC/persistence errors | Partial | Partial | Partial | D01/D04/D05; lock/disk failures remain |

A language that fails a hard gate cannot be selected, even if its weighted
score is highest.

Passes currently refer to the Fedora x86_64 and macOS preflight evidence sets.
They remain provisional until the source tree is clean and T03/T04 are reviewed.
Short-lived task state is detected, but asynchronous exit codes are not yet
persisted and timeout/restart-policy tests remain.

## Required Conclusions

1. Evaluation summary
2. Test environment
3. Test results
4. Performance comparison
5. Dependency assessment
6. Engineering complexity
7. Architecture risks
8. Language comparison
9. Recommended language
10. Reasons for rejecting alternatives
11. Remaining unknowns

## Architecture Revisions

Record invalid assumptions, required object-model changes, follow-up POCs, and
the proposed Baseline v0.3 changes.

