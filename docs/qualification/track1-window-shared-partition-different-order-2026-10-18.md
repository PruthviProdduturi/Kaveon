# Track 1 — Shared partition windows with independent orderings

## Scope

Window expressions may use different `ORDER BY` clauses while sharing the same `PARTITION BY` keys. They can safely share one hash exchange; each worker must then evaluate each ordered frame independently within the complete partition.

## Evidence

```text
cargo test -p kaveon-server --bin kaveon-server planner::tests::shares_partition_exchange_when_window_orderings_differ --quiet
cargo fmt --package kaveon-server
cargo clippy -p kaveon-server --all-targets -- -D warnings
```

The planner regression combines `ROW_NUMBER()` ordered by `actions` with a bounded `SUM()` frame ordered by `country`, both partitioned by `region`. It requires one four-task hash exchange on `region` and a distributed `PartitionedWindow` stage.

## Boundary

This qualifies planning for shared partition keys with different orderings. A live multi-worker result differential for this exact shape remains required before broadening the distributed-window claim.
