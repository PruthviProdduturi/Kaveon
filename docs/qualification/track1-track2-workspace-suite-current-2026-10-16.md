# Track 1 and Track 2 workspace qualification — current source

**Date:** 2026-10-16  
**Source:** `origin/dev` at `4c049037` (the workspace gate includes the
correlated array-function, map-key, map-entry, per-row map-concat, named
window, JSON object-constructor, JSON null-clause, and bounded recursive CTE
regressions)

The complete Rust workspace test command was run from `engine/`:

```text
cargo test --workspace --all-targets
```

The command completed with **1,539 listed tests, 1,533 passed, 0 failed** across all
workspace test binaries; the run also reported the repository's six
intentional/benchmark ignores. The long server differential sweep completed
successfully. Component totals were 74 catalog, 212 CLI, 87 core, 380
execution, 68 optimizer, 410 server, 183 SQL, and 125 storage tests.

The release lint commands also passed:

```text
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

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

This is repository-level evidence only. It does not close the remaining Track
1 AKS gates or Track 2 compatibility boundaries; those remain listed in the
parity trackers and require their corresponding live or differential evidence.
