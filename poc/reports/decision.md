# Language Decision Report

Status: pending POC results.

## Hard Gates

| Gate | Rust | Go | C++ | Evidence |
|---|---|---|---|---|
| Basic terminal interaction | Not run | Not run | Not run | |
| Reliable process-group control | Not run | Not run | Not run | |
| Tasks survive daemon crash | Not run | Not run | Not run | |
| Recoverable TaskInstance identity | Not run | Not run | Not run | |
| Acceptable dependency health | Not run | Not run | Not run | |
| Explicit IPC/persistence errors | Not run | Not run | Not run | |

A language that fails a hard gate cannot be selected, even if its weighted
score is highest.

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

