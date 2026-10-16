# Track 1 and Track 2 workspace qualification — current source

**Date:** 2026-10-16  
**Source:** `origin/dev` at `e6c13819` (the workspace gate includes the
correlated array-function, map-key, map-entry, per-row map-concat, named
window, JSON object-constructor, JSON null-clause, and bounded recursive CTE
regressions)

The complete Rust workspace test command was run from `engine/`:

```text
cargo test --workspace --all-targets
```

The command completed with **1,543 listed tests, 1,537 passed, 0 failed** across all
workspace test binaries; the run also reported the repository's six
intentional/benchmark ignores. The long server differential sweep completed
successfully. Component totals were 74 catalog, 212 CLI, 87 core, 382
execution, 68 optimizer, 412 server, 183 SQL, and 125 storage tests.

The release lint commands also passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

The workspace Clippy gate was rerun after the TopN change and exited with code
0 in 12.06 seconds.

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
1 passed, 0 failed (410 filtered out; 135.21s)
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
array/map/row keys; the focused sweep passed **1/1** after that extension
(411 filtered server tests; 135.24 seconds).

After that change, the complete server target suite was rerun from the same
source:

```text
cargo test -p kaveon-server --all-targets
412 passed, 0 failed, 0 ignored (139.53s)
```

This includes the planner regression for projected-key `UNION DISTINCT` and
the existing distributed join, aggregate, spill, retry, cancellation, and
worker-loss coverage.

This is repository-level evidence only. It does not close the remaining Track
1 AKS gates or Track 2 compatibility boundaries; those remain listed in the
parity trackers and require their corresponding live or differential evidence.
