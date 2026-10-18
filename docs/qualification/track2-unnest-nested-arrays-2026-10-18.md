# Track 2: nested array UNNEST

## Contract

`UNNEST` preserves an array-valued element as one row value. A nested array
must not be flattened implicitly, and the inner list remains available to
array functions after the distributed fragment exchange.

## Exact qualification

```sql
SELECT CARDINALITY(u.value) AS element_count
FROM UNNEST(ARRAY[ARRAY[1, 2], ARRAY[3]]) AS u(value)
ORDER BY element_count
```

Expected and observed result: `1`, `2` from both the local planner and the
three-worker in-process distributed fragment path.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-server distributed_nested_array_unnest_preserves_inner_lists -- --nocapture
```
