# Track 1 and Track 2 workspace qualification — current source

**Date:** 2026-10-16  
**Source:** `origin/dev` at `df894fbe` (the workspace gate includes the
correlated array-function, map-key, map-entry, per-row map-concat, named
window, JSON object-constructor, JSON null-clause, and bounded recursive CTE
regressions, plus nested null-safe array/map/row comparison, LargeList map
constructors, primitive numeric map keys, date/decimal map keys, decimal
map-key regression coverage, explicit, wildcard-derived, nested-derived,
derived column-list aliases, equality-correlated, and uncorrelated-lateral
`JOIN ... USING`)

The complete Rust workspace test command was run from `engine/`:

```text
cargo test --workspace --all-targets --no-fail-fast --quiet
```

The command completed with **1,563 listed tests, 1,557 passed, 0 failed, and 6
ignored** across all workspace test binaries. The long server differential
sweep completed successfully. Component totals were 74 catalog, 212 CLI, 87
core, 386 execution, 69 optimizer, 412 server, 185 SQL, and 125 storage tests.

The release lint commands also passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

The workspace Clippy gate exited with code 0.

Against the current source above, `cargo fmt --all -- --check` and
`cargo clippy --locked --workspace --all-targets -- -D warnings` both exited
with code 0.

After the workspace evidence was captured, the planner regression for
unpartitioned windows was added and the current source was rechecked with the
two focused gates below:

```text
cargo test -p kaveon-sql bracketed_identifiers -- --nocapture
1 passed, 0 failed

cargo test -p kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
1 passed, 0 failed (411 filtered out; 140.31s)
```

The bracketed identifier check covers the Studio-generated
`SELECT DISTINCT [year] ...` form that previously surfaced as an `ARRAY`
function error. The differential gate covers the distributed SQL corpus over
the current parser and planner.

The TopN operator gate also passes **17/17** focused tests, including a
sorted-string worker merge. That path now merges already-sorted non-integer
and multi-key candidates by their heads while retaining the existing Arrow
fallback for unsorted input.

The distributed differential sweep was rerun after the planner change and
passed **1/1** (411 filtered server tests; 136.86 seconds). It includes a
projected-key `UNION DISTINCT` case whose branches now hash-partition across
workers before the final deduplication; unsupported expression-shaped unions
still use the single-task fallback.

The sweep also covers typed DATE and DECIMAL set operations. The set operator
and DISTINCT key paths now canonicalize DATE32/DATE64/TIMESTAMP, DECIMAL128,
narrow integer/Float32, and LargeUtf8 values, with deterministic nested
array/map/row keys; focused array, map, and row set-key tests pass, and the sweep passed **1/1** after that extension
(411 filtered server tests; 135.24 seconds).

The differential corpus now also includes SQL-level array and map
`INTERSECT` cases plus a composite multi-column row `EXCEPT` case; the exact
local/distributed sweep passed **1/1** after those additions (411 filtered
server tests; 134.00 seconds). The direct-map, correlated-map, and mixed
map-plus-array UNNEST cases also pass in the same sweep: `UNNEST(MAP(...))`
is normalized to `MAP_ENTRIES`, the lateral operator expands each struct entry
into aliased key/value columns, and mixed expressions preserve padding and
ordinality (411 filtered server tests; 133.68 seconds).

The map cardinality case also passed in the exact-result sweep (411 filtered
server tests; 134.19 seconds), covering `CARDINALITY(MAP(...))` across the
local and distributed execution paths.

The optimizer rejection corpus now also has a focused binder test for a
correlated scalar `OFFSET ... LIMIT` with a residual predicate. It fails closed
with the explicit Apply-required diagnostic rather than silently rebinding the
outer columns.

After that change, the complete server target suite was rerun from the same
source:

```text
cargo test -p kaveon-server --all-targets --no-fail-fast --quiet
412 passed, 0 failed, 0 ignored (145.30s)
```

This includes the planner regression for projected-key `UNION DISTINCT` and
the existing distributed join, aggregate, spill, retry, cancellation, and
worker-loss coverage.

The exchange-focused regression also verifies that nested array, map, and row
keys partition successfully, including two maps with reversed entry order
landing in the same partition; map/row keys use the canonical nested fallback
because Arrow's `RowConverter` does not implement Map values.

The complete execution crate target suite was also rerun on this source:

```text
cargo test -p kaveon-exec --all-targets --no-fail-fast --quiet
382 passed, 0 failed, 4 ignored (14.83s)
```

This is repository-level evidence only. It does not close the remaining Track
1 AKS gates or Track 2 compatibility boundaries; those remain listed in the
parity trackers and require their corresponding live or differential evidence.
