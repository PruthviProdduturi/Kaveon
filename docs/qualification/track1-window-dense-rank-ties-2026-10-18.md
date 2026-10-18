# Track 1 — distributed partitioned `DENSE_RANK` ties

## Scope

The server differential gate now compares a partitioned `DENSE_RANK()` window
with ties between the local and three-worker execution paths. The result is
canonicalized after the same final ordering and limit, so equal rank values
must retain the same peer rows on every path.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server distributed_partitioned_window_dense_rank_ties_matches_local -- --nocapture
test result: ok; 1 passed; 0 failed
```

The query partitions by `region`, orders by descending `actions`, and returns
the first 24 rows ordered by region, rank, and country. The local and three
worker results match exactly and the query memory pool returns to zero after
both executions.

## Boundary

This qualifies exact partitioned `DENSE_RANK` peer handling on the declared
three-worker in-process topology. It does not claim parallel execution for
unpartitioned windows, cloud qualification, or broader window endurance.
