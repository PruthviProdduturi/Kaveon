# Track 1 — distributed partitioned `PERCENT_RANK` and `CUME_DIST`

## Scope

The server differential gate compares partitioned `PERCENT_RANK()` and
`CUME_DIST()` windows between the local planner and the three-worker execution
path. The ordering contains nullable `industry` values and repeated
`actions` values so peer groups and `NULLS FIRST` boundaries are exercised.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server differential_tests::distributed_partitioned_window_percent_rank_and_cume_dist_match_local -- --exact --nocapture
test result: ok; 1 passed; 0 failed
```

The local and three-worker results match exactly, including the shared rank
for peer rows, cumulative peer-group boundary, partition-local denominator,
nullable ordering, and zero memory retained after both executions.

## Boundary

This qualifies bounded partitioned percent-rank and cumulative-distribution
semantics on the declared three-worker in-process topology. Unpartitioned
global window exchange remains centralized; this evidence does not claim an
ordered Arrow frontier protocol, cloud qualification, or broad window
endurance.
