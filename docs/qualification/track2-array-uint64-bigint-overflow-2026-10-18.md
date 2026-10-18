# Track 2: UINT64 to BIGINT overflow boundary

## Contract

When an ARRAY is promoted to BIGINT, a UINT64 value above `i64::MAX` must never wrap into a negative BIGINT. The direct evaluator rejects that conversion with `UINT64 value is outside BIGINT range`.

At the SQL literal boundary, an out-of-range UINT64 cast is represented as a NULL element by the existing checked cast path. Local and distributed execution preserve that NULL and the valid neighboring BIGINT value identically.

## Exact qualification

```sql
SELECT ARRAY[
  CAST(18446744073709551615 AS UINT64),
  CAST(2 AS BIGINT)
] AS values
FROM events
LIMIT 1
```

The local planner and the in-process three-worker distributed fragment path both return the null-preserving list `[, 2]`; no wrapped negative value is emitted. The direct evaluator regression verifies the explicit fail-closed error for an in-memory UINT64 array that reaches the promotion path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-exec array_constructor_rejects_uint64_outside_bigint_range -- --nocapture
cargo test -p kaveon-server distributed_array_uint64_outside_bigint_range_fails_closed -- --nocapture
```
