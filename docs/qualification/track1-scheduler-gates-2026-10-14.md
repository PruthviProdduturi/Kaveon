# Track 1 scheduler gates: skew, retry, and stale attempts

**Date:** 2026-10-14  
**Scope:** deterministic scheduler behavior exercised in the Engine test
crate. These focused tests complement the live Docker and AKS topology reports.

From `engine/`:

```powershell
cargo test -p kaveon-server stealing_a_skewed_tail_invalidates_the_original_attempt --quiet
cargo test -p kaveon-server failed_leases_return_splits_to_the_queue --quiet
cargo test -p kaveon-server does_not_retry_a_worker_for_the_same_partition --quiet
```

All three tests passed. The assertions prove that:

- a skewed tail can be stolen by an idle worker;
- the replacement attempt increments the attempt number;
- the original attempt is rejected after the steal;
- a failed lease returns its splits to the queue for a different worker; and
- retry candidates do not assign the same partition to the same worker.

This is scheduler-level evidence. It does not claim that a live AKS run
experienced a skewed tail or stale attempt; that remains a live fault-injection
qualification requirement.

## Clean-checkout rerun — 2026-10-15

The three focused gates were rerun from the clean Engine checkout used for the
current pushed baseline. `stealing_a_skewed_tail_invalidates_the_original_attempt`,
`failed_leases_return_splits_to_the_queue`, and
`does_not_retry_a_worker_for_the_same_partition` each passed (`1 passed,
0 failed`).
