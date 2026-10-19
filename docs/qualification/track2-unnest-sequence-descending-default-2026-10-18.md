# Track 2: descending default `SEQUENCE` in `UNNEST`

## Scope

When the start is greater than the stop and no step is supplied, Trino infers
a unit descending step:

```sql
SELECT value, ord
FROM UNNEST(SEQUENCE(3, 1)) WITH ORDINALITY AS t(value, ord)
ORDER BY ord
```

## Evidence

`differential_tests::distributed_sequence_unnest_infers_descending_default_step`
compares local planning with the three-worker fragment path. Both return:

```text
3 | 1
2 | 2
1 | 3
```

The query memory pool returns to zero after execution.

Command:

```text
cargo test -p kaveon-server distributed_sequence_unnest_infers_descending_default_step -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This qualifies the constant descending default-step form only. Dynamic
sequence bounds, unsupported step types, arbitrary table functions, and
external/cloud qualification remain open.
