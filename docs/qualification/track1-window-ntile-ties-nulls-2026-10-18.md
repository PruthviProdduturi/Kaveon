# Track 1 — distributed partitioned `NTILE` ties and NULL ordering

## Scope

The server differential gate compares partitioned `NTILE(3)` and `NTILE(5)`
windows between the local planner and the three-worker execution path. The
window ordering includes nullable `industry`, repeated values (peer ties), and
secondary keys, while the final result is explicitly ordered for comparison.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server differential_tests::distributed_partitioned_window_ntile_ties_and_nulls_matches_local -- --exact --nocapture
test result: ok; 1 passed; 0 failed
```

The test covers larger buckets first, both bucket counts, `NULLS FIRST`,
repeated nullable ordering values, partition-local numbering, and exact local
versus three-worker results. The query memory pool returns to zero after both
executions.

## Boundary

This qualifies bounded partitioned NTILE evaluation on the declared
three-worker in-process topology. Unpartitioned global window exchange remains
centralized; this result does not claim an ordered Arrow frontier protocol,
cloud qualification, or broader window endurance.
