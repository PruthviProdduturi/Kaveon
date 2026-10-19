# Track 2 qualification: `COMBINATIONS`

Date: 2026-10-19

## Contract

`COMBINATIONS(array, n)` returns the ordered `n`-element subsets of each input
array as a nested array. Input order is preserved within each combination and
between combinations. A NULL array or NULL `n` produces a NULL row; `n = 0`
produces one empty combination; `n` greater than the input cardinality produces
an empty array.

The implementation follows Trino's safety bounds: `n` must be between 0 and 5,
and the flattened result is capped at 100,000 values per input row.
Oversized requests fail closed before constructing an unbounded nested result.
The planner exposes the result as `ARRAY<ARRAY<T>>`, including for empty and
NULL rows, so downstream projection and exchange retain the element type.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-exec --lib combinations_
```

The evaluator tests cover ordered `BIGINT` combinations, NULL input rows,
`n = 0`, and the upper-bound rejection. The distributed differential corpus
also compares `COMBINATIONS(ARRAY[1, 2, 3], 2)` across local and three-worker
execution paths.

External-client and cloud qualification remain part of the broader Track 2
boundary.
