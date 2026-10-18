# Track 1 scheduler lease lifecycle qualification — 2026-10-18

## Scope

This qualification covers the local `SplitScheduler` lease lifecycle. Each
lease records its worker and heartbeat timestamp. A current worker can refresh
its lease; an expired lease is removed and requeued exactly once; retries are
served in partition order; and a takeover increments the task attempt so late
results from the previous worker fail the existing task identity fence.

This is a local scheduler qualification. It does not claim cloud or remote
split enumeration, node-loss endurance, or AKS qualification.

## Evidence

Command, from `engine/`:

```text
cargo test -p kaveon-server scheduler -- --nocapture
```

Result: **14 passed, 0 failed**. The three lifecycle cases are:

- heartbeat refresh prevents premature expiry and expiry requeues once;
- a healthy lease cannot be stolen, while an expired lease can be taken over;
- multiple expired partitions produce deterministic retry order.

The existing scheduler tests also cover deterministic local split enumeration,
retry batch preservation, worker rotation, and stale completion rejection.
