# Track 2: typed NULL array padding in `UNNEST`

## Scope

Trino's multi-array `UNNEST` keeps the longest non-null array's row count and
NULL-pads a typed NULL sibling:

```sql
SELECT *
FROM UNNEST(
  ARRAY[1, 2],
  CAST(NULL AS ARRAY<VARCHAR>)
) WITH ORDINALITY AS t(id, label, ord)
ORDER BY ord
```

Kaveon now accepts the typed array cast, preserves its element type through
planning, and allows it as a row-producing UNNEST input. The existing Arrow
cast path materializes the NULL list with the declared element type.

## Evidence

`differential_tests::distributed_multi_array_unnest_typed_null_array_pads_rows`
compares local execution with the three-worker fragment path. Both return:

```text
1 | NULL | 1
2 | NULL | 2
```

The query memory pool returns to zero after both executions.

Command:

```text
cargo test -p kaveon-server distributed_multi_array_unnest_typed_null_array_pads_rows -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This covers typed NULL arrays used as multi-array UNNEST inputs. Sized array
casts, untyped `ARRAY` casts, arbitrary table functions, and external/cloud
qualification remain outside this bounded item.
