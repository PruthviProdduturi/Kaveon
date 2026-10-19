# Track 2: typed NULL MAP in `UNNEST`

## Scope

A typed NULL map cast is a valid row-producing input. Trino exposes map entries
as key/value columns and returns an empty relation when the map itself is NULL:

```sql
SELECT key, value
FROM UNNEST(CAST(NULL AS MAP(VARCHAR, BIGINT))) AS t(key, value)
```

Kaveon now normalizes the typed map cast through its existing `MAP_ENTRIES`
representation before the UNNEST operator runs.

## Evidence

`differential_tests::distributed_unnest_typed_null_map_is_empty_with_schema`
compares local execution with the three-worker fragment path. Both paths bind
the key/value schema and return zero rows; the query memory pool returns to
zero after execution.

Command:

```text
cargo test -p kaveon-server distributed_unnest_typed_null_map_is_empty_with_schema -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This covers unsized typed NULL MAP casts as standalone UNNEST inputs. Non-null
typed map casts, sized casts, arbitrary table functions, and external/cloud
qualification remain open.
