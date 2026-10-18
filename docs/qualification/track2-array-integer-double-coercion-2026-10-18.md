# Track 2: integer and DOUBLE ARRAY coercion

## Contract

An `ARRAY` containing integer and DOUBLE expressions promotes the common numeric type to DOUBLE. Integer values are converted exactly before list construction, matching Trino's numeric coercion behavior.

## Exact qualification

```sql
SELECT ELEMENT_AT(
  ARRAY[CAST(1 AS INTEGER), CAST(2.5 AS DOUBLE)],
  2
) AS value
FROM events
LIMIT 1
```

The local planner and the in-process three-worker distributed fragment path both return `2.5`. The array element type is DOUBLE (`Float64`) before `ELEMENT_AT` extracts the second value.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-exec array_constructor_promotes_integer_and_double_to_double -- --nocapture
cargo test -p kaveon-server distributed_array_integer_and_double_promote_to_double -- --nocapture
```
