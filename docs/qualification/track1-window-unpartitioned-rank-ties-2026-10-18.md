# Track 1 unpartitioned rank ties — 2026-10-18

## Boundary

The distributed differential corpus now covers unpartitioned `RANK()` and
`DENSE_RANK()` over the same global ordering, including peer ties. The result
is compared against the local planner on the declared three-worker harness.
This protects the centralized global-order window boundary while parallel
unpartitioned window execution remains a separate target.

## Evidence

```text
cargo test -p kaveon-server distributed_unpartitioned_rank_ties_match_local -- --nocapture
```

Result: 1 passed, 0 failed. The test confirms equal rows and rank values and
returns the query memory pool to zero after both executions.

## Remaining boundary

Unpartitioned windows remain centralized for correctness. Parallel execution
with global rank state and distributed peer-boundary handling is not claimed by
this qualification.
