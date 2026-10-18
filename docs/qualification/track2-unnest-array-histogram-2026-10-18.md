# Track 2 qualification: direct `UNNEST(ARRAY_HISTOGRAM(...))`

Date: 2026-10-18

## Contract

The row-producing `UNNEST` surface accepts the map returned by
`ARRAY_HISTOGRAM`, exposing its keys and counts as two columns with the same
ordering and typed values as Trino. The table-function allow-list remains
explicit; this does not enable arbitrary scalar functions in `UNNEST`.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-server distributed_array_histogram_unnest_matches_trino_rows --quiet
```

The distributed three-worker regression evaluates
`UNNEST(ARRAY_HISTOGRAM(ARRAY[2, 1, 2]))` and returns `(1, 1)` and `(2, 2)`.
Broader table-function and external-client qualification remains open in the
parity matrix.
