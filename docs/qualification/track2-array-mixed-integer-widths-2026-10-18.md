# Track 2: mixed integer ARRAY promotion

## Contract

An `ARRAY` containing integer expressions with different Arrow widths promotes the common numeric type to BIGINT, matching Trino's integer coercion rule. Narrow integer values remain exact when combined with BIGINT values.

## Exact qualification

```sql
SELECT ELEMENT_AT(
  ARRAY[CAST(1 AS INTEGER), CAST(2 AS BIGINT)],
  2
) AS value
FROM events
LIMIT 1
```

The local planner and the in-process three-worker distributed fragment path both return `2`. The array element type is BIGINT (`Int64`) before `ELEMENT_AT` extracts the second value.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-exec array_constructor_promotes_mixed_integer_widths_to_bigint -- --nocapture
cargo test -p kaveon-server distributed_array_mixed_integer_widths_promote_to_bigint -- --nocapture
```
