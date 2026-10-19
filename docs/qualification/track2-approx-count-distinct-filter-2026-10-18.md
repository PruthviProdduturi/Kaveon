# Track 2: approximate distinct aggregate FILTER parity

## Scope

This qualification covers `APPROX_COUNT_DISTINCT(value) FILTER (WHERE predicate)`
inside a grouped query. The filtered input is evaluated before the approximate
state is updated, preserving Trino's NULL and predicate semantics across local,
partitioned, and three-worker execution.

## Evidence

- `KAVEON_DIFF_ONLY=approx_count_distinct_filter cargo test -p kaveon-server --bin kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture`
  passed on 2026-10-18. The same exact result was observed for dictionary/plain,
  multi-file, partitioned, clustered, and in-process three-worker execution.

## Boundary

This qualifies only approximate distinct with `FILTER`; approximate percentile
filters and the broader aggregate-function corpus remain separate boundaries.