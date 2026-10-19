# Track 2: nested array values in MAP casts

Date: 2026-10-18  
Scope: local and in-process three-worker differential execution

## Contract

The bounded complex-cast surface accepts a Trino-compatible map whose values
are nested arrays, for example:

```sql
SELECT ELEMENT_AT(
  ELEMENT_AT(
    ELEMENT_AT(
      CAST(
        MAP(ARRAY['numbers'], ARRAY[ARRAY[ARRAY[1, 2]]])
        AS MAP(VARCHAR, ARRAY(ARRAY(BIGINT)))
      ),
      'numbers'
    ),
    1
  ),
  2
)
```

The cast recursively converts both list levels while preserving their offsets
and validity. The parser admits at most two nested array levels in this map
fallback; map, row, and deeper recursive members remain fail-closed until
separately qualified.

## Evidence

`distributed_nested_array_map_cast_matches_local_rows` in
`engine/crates/server/src/differential_tests.rs` evaluates the query locally
and through the three-worker fragment path. Both paths return the exact row
`2`, and the query memory pool returns to zero after execution.

The SQL planner regression
`parses_bounded_nested_array_map_values` verifies the nested
`MAP(VARCHAR, ARRAY(ARRAY(BIGINT)))` target, while the parser rejection test
keeps triple-nested arrays and unsupported map/row members outside this
bounded contract.

## Validation

```text
cargo test -p kaveon-sql bounded_nested_array_map_values -- --nocapture
cargo test -p kaveon-server --bin kaveon-server \
  distributed_nested_array_map_cast_matches_local_rows -- --nocapture
```

Both tests passed on the 2026-10-18 integration worktree.
