# Track 2: empty typed NULL-array `UNNEST` schema

## Scope

A typed NULL array used as the only `UNNEST` input must bind its element
schema and produce an empty relation:

```sql
SELECT label
FROM UNNEST(CAST(NULL AS ARRAY<VARCHAR>)) AS t(label)
```

## Evidence

`differential_tests::distributed_unnest_typed_null_array_is_empty_with_schema`
plans the query locally and executes the same plan through three workers. Both
paths produce zero rows, preserve the declared VARCHAR output column, and
release all query memory.

Command:

```text
cargo test -p kaveon-server distributed_unnest_typed_null_array_is_empty_with_schema -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This qualifies an unsized typed NULL array as a standalone UNNEST input. Sized
array casts, untyped array casts, arbitrary table functions, and
external/cloud qualification remain open.
