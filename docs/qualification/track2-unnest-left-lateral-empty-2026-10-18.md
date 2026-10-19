# Correlated `LEFT JOIN LATERAL UNNEST` qualification

This qualification covers the Trino-compatible outer lateral array shape:

```sql
SELECT x.id, u.value, u.ord
FROM (VALUES
  (1, CAST(NULL AS ARRAY<BIGINT>)),
  (2, ARRAY[20, 30])
) AS x(id, arr)
LEFT JOIN LATERAL UNNEST(x.arr)
  WITH ORDINALITY AS u(value, ord) ON TRUE
ORDER BY x.id, u.ord;
```

The empty or NULL array preserves its left row with NULL element and NULL
ordinality. Non-empty arrays retain one-based ordinality. The local planner and
the in-process three-worker fragment path return the same exact rows:

```text
1 | NULL | NULL
2 | 20   | 1
2 | 30   | 2
```

The regression is
`distributed_left_correlated_lateral_unnest_null_extends_empty_arrays` in
`engine/crates/server/src/differential_tests.rs`. The operator keeps the
existing `CROSS JOIN LATERAL UNNEST` behavior and enables NULL extension only
for `LEFT JOIN ... ON TRUE`.

This qualifies correlated array expressions and `WITH ORDINALITY` through
local and in-process distributed execution. It does not claim arbitrary table
functions, lateral limits, residual `ON` predicates, external-client wire
qualification, or cloud qualification.
