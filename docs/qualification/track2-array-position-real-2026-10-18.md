# Track 2 qualification: `ARRAY_POSITION` over `REAL` arrays

The evaluator previously handled `ARRAY_POSITION` for `DOUBLE` and several
other primitive types but rejected an Arrow `Float32` list. Trino's `REAL`
type maps to that Arrow type, so the omission was a compatibility gap.

The evaluator now casts the needle to `Float32` and compares it against the
list's native values. It does not widen the list to `DOUBLE`, preserving the
exact `REAL` equality boundary. The focused regression uses two adjacent
`Float32` values and checks both positions:

```text
cargo test -p kaveon-exec evaluates_array_position_for_real_lists_without_double_widening -- --nocapture
```

Result: **1 passed, 0 failed**.

The current differential corpus also includes the SQL case
`ARRAY_POSITION(ARRAY[CAST(... AS REAL), ...], CAST(... AS REAL))` in the
local/distributed Parquet-layout sweep. The full sweep passed with **1 passed,
0 failed** in 378.65 seconds (446 filtered tests), including dictionary,
plain, multi-file, partitioned, clustered, and in-process three-worker
execution paths. External pinned-Trino and cloud qualification remain outside
this focused change.
