# Track 2: DECIMAL and DOUBLE ARRAY coercion

## Contract

An `ARRAY` containing DECIMAL and DOUBLE expressions promotes the common numeric type to DOUBLE. Decimal values are converted through the checked floating-point cast before list construction.

## Exact qualification

```sql
SELECT ELEMENT_AT(
  ARRAY[CAST(1.25 AS DECIMAL(10, 2)), CAST(2.5 AS DOUBLE)],
  2
) AS value
FROM events
LIMIT 1
```

The local planner and the in-process three-worker distributed fragment path both return `2.5`. The array element type is DOUBLE (`Float64`) before `ELEMENT_AT` extracts the second value. Decimal precision and scale beyond DOUBLE's exact range remain subject to floating-point semantics.

## Reproduce

From `engine/`:

```powershell
cargo test -p kaveon-exec array_constructor_promotes_decimal_and_double_to_double -- --nocapture
cargo test -p kaveon-server distributed_array_decimal_and_double_promote_to_double -- --nocapture
```
