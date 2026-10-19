# Track 2: nested typed NULL-array schema in `UNNEST`

## Scope

Recursive array cast targets must preserve the nested element schema when the
outer array is NULL:

```sql
SELECT value
FROM UNNEST(CAST(NULL AS ARRAY<ARRAY<BIGINT>>)) AS t(value)
```

## Evidence

`differential_tests::distributed_unnest_nested_typed_null_array_is_empty_with_schema`
compares local execution with the three-worker fragment path. Both paths bind
the nested list output and return zero rows; all query memory is released.

Command:

```text
cargo test -p kaveon-server distributed_unnest_nested_typed_null_array_is_empty_with_schema -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This qualifies an unsized nested typed NULL-array input. Sized array casts,
untyped array casts, arbitrary table functions, and external/cloud
qualification remain open.
