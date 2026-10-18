# Track 1 — unpartitioned window global order

## Scope

The distributed differential gate now verifies that an unpartitioned
`ROW_NUMBER()` window retains one global ordering across three workers. The
centralized window path is compared with the local reference before the final
ordering and limit are applied.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-server distributed_unpartitioned_window_preserves_global_row_number_order -- --nocapture
test result: ok; 1 passed; 0 failed
```

The query orders by `actions DESC, country`, assigns `ROW_NUMBER()` without a
partition key, and returns the first 24 rows. Local and three-worker results
match exactly, and the query memory pool returns to zero after both paths.

## Boundary

This qualifies global-order correctness for the current centralized
unpartitioned window path. It does not claim parallel unpartitioned window
execution, cloud qualification, or broader endurance.
