# Track 2: temporal MAP cast members in `UNNEST`

## Scope

`MAP(key_type, value_type)` casts now accept temporal key members that the map
executor already supports. This closes the planner/parser gap for a typed NULL
map with a `DATE` key:

```sql
SELECT key, value
FROM UNNEST(CAST(NULL AS MAP(DATE, BIGINT))) AS t(key, value)
```

The typed NULL map binds the key/value schema and produces an empty relation
through both the local evaluator and the three-worker fragment path.

## Evidence

Command:

```text
cargo test -p kaveon-server distributed_unnest_typed_null_date_map_is_empty_with_schema -- --nocapture
```

Result: 1 passed, 0 failed.

## Boundary

This change covers `DATE`, `TIMESTAMP`, and `TIMESTAMP WITH TIME ZONE` member
names in the custom `MAP(...)` cast syntax. Decimal precision/scale members,
nested complex cast members, and external/cloud qualification remain open.
