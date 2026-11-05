# Experiment 0012: Stage 3 Closure

Status: Complete
Date: 2026-11-05
Closure revision: `467e96f` (`dev`)

## Exit Criteria

| Requirement | Evidence |
|---|---|
| Ownership matrix, reclaim token, and grace | AUTH probes and `qwe-core` state-policy tests |
| Detached hold and Lease release tombstone | `stage3-attach-authority` |
| Unreachable, lost, orphan, and conflict reconciliation | LIVE/HIST probes and T12 |
| Durable idempotency and response replay | `stage3-idempotency` |
| Lease clock, TTL, and retention policy | ADR 0016 and storage tests |
| macOS and Fedora matrix | `just stages-1-3-check` |
| Duplicate requests do not create duplicate Tasks | pending/complete idempotency recovery probe |
| Crash recovery preserves ownership and Task/Lease history | reclaim, restart, reconciliation, and resilience probes |

## Final Verification

- `just stages-1-3-check`: pass on macOS ARM64 and Fedora Linux x86_64.
- Independent Stage 2 T03/T04/T08/T11 review: pass on both platforms.
- `ATTACH-015` terminal keyboard reset: pass on both platforms.
- macOS T12 RSS: 47,392 KiB.
- Fedora T12 RSS: 37,556 KiB.

## Reliability Observation

One Fedora full-suite rerun during closure produced:

```text
worker_unreachable: worker control request timed out
```

The immediate isolated T12 run passed, the complete Stage 1-3 suite then
passed, and four consecutive Fedora T12 repetitions passed. No product change
was made to hide the event. It remains a low-frequency platform reliability
watch item for Stage 4 service/upgrade testing.

## Stage 4 Isolation

The incomplete Stage 4 credential scaffolding is isolated on:

```text
codex/stage-4-services-credentials
9ffde85 wip(stage4): scaffold credential descriptor injection
```

It is not merged into `dev` and is not part of this Stage 3 closure.
