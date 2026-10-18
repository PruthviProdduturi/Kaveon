# Track 2: UINT32 and BIGINT ARRAY coercion

## Contract

An `ARRAY` containing a UINT32 and a BIGINT promotes the common numeric type to BIGINT. UINT32 values are widened exactly; unsigned values outside BIGINT range remain rejected by the checked cast path.

## Exact qualification

```sql
SELECT ELEMENT_AT(
  ARRAY[CAST(1 AS UINT32), CAST(2 AS BIGINT)],
  2
) AS value
FROM events
LIMIT 1
```

The local planner and the in-process three-worker distributed fragment path both return `2`. The array element type is BIGINT (`Int64`) before `ELEMENT_AT` extracts the second value.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-exec array_constructor_promotes_uint32_and_bigint_to_bigint -- --nocapture
cargo test -p kaveon-server distributed_array_uint32_and_bigint_promote_to_bigint -- --nocapture
```
