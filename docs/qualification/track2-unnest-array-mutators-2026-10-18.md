# Track 2: row-producing `UNNEST` array mutators

Kaveon now accepts the bounded Trino-compatible array mutators
`ARRAY_APPEND`, `ARRAY_PREPEND`, and `ARRAY_REMOVE` as row-producing inputs to
`UNNEST`.

Examples:

```sql
SELECT * FROM UNNEST(ARRAY_APPEND(ARRAY[1, 2], 3)) AS t(value);
SELECT * FROM UNNEST(ARRAY_PREPEND(ARRAY[1, 2], 3)) AS t(value);
SELECT * FROM UNNEST(ARRAY_REMOVE(ARRAY[1, 2, 3], 3)) AS t(value);
```

The parser allow-list remains explicit: scalar or arbitrary table-function
expressions are still rejected before planning. The executor reuses the
existing columnar list expansion and therefore retains the same padding,
ordinality, and lateral behavior as other supported array inputs.

Validation on 2026-10-18:

- `cargo test -p kaveon-sql parses_array_append_prepend_and_remove_unnest_in_from`
- `cargo test -p kaveon-exec expands_array_mutators_as_row_producing_inputs`
- `cargo fmt --check`
- `cargo clippy -p kaveon-exec -p kaveon-sql --all-targets -- -D warnings`

This is local qualification only. Distributed and external-client wire
coverage for the three mutators remains open with the broader row-producing
`UNNEST` corpus.
