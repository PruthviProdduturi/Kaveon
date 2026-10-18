# Track 2 qualification: `ARRAY_POSITION` over `INTEGER` arrays

The evaluator previously handled `BIGINT` and `REAL` arrays but rejected a
regular Arrow `List<Int32>`. Trino `INTEGER` maps to Arrow `Int32`, so this
was a bounded SQL compatibility gap for typed array expressions.

The evaluator now casts the needle to `Int32` and compares it at the list's
native width. The focused executor regression covers two rows and verifies the
one-based positions:

```text
cargo test -p kaveon-exec evaluates_array_position_for_integer_lists -- --nocapture
```

The local and in-process three-worker differential case is selected with:

```text
$env:KAVEON_DIFF_ONLY = "integer_array_position"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

This qualification covers regular `INTEGER` lists and the bounded local and
distributed fragment paths. Other narrow integer element types and external
client/cloud qualification remain outside this focused change.
