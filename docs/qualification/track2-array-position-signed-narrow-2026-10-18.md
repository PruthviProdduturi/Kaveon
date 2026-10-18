# Track 2 qualification: `ARRAY_POSITION` over signed narrow arrays

`ARRAY_POSITION` now accepts regular Arrow `List<Int8>` and `List<Int16>`
values and compares each needle at the list's native width. SQL `TINYINT`
and `SMALLINT` casts map to those signed Arrow widths, so this coverage
exercises parsing, cast planning, local evaluation, and the in-process
three-worker fragment path.

The focused local regressions are:

```text
cargo test -p kaveon-exec evaluates_array_position_for_int8_lists -- --nocapture
cargo test -p kaveon-exec evaluates_array_position_for_int16_lists -- --nocapture
```

The exact local and three-worker differential cases are selected with:

```text
$env:KAVEON_DIFF_ONLY = "int8_array_position,int16_array_position"
cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
```

The cases include the signed minimum values (`-128` and `-32768`), a missing
value in the list, and one-based position results.
