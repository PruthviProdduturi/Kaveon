# Track 2: correlated `ARRAY_REPEAT` in lateral `UNNEST`

## Scope

An allow-listed array function in a lateral relation must be evaluated for
each outer row, including its repeat count:

```sql
SELECT x.id, u.value
FROM (VALUES (1), (3)) AS x(id)
CROSS JOIN LATERAL UNNEST(ARRAY_REPEAT(x.id, x.id)) AS u(value)
ORDER BY x.id, u.value
```

## Evidence

`differential_tests::distributed_correlated_array_repeat_unnest_uses_outer_count`
compares local execution with the three-worker fragment path. Both return one
`1` for `x.id = 1` and three `3` values for `x.id = 3`; the query memory pool
returns to zero.

Command:

```text
cargo test -p kaveon-server distributed_correlated_array_repeat_unnest_uses_outer_count -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This qualifies correlated `ARRAY_REPEAT` with scalar outer-row arguments. Other
array functions, arbitrary table functions, deeper lateral shapes, and
external/cloud qualification remain open.
