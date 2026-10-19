# Track 2 scalar `APPROX_PERCENTILE` `FILTER` qualification — 2026-10-18

The bounded scalar and percentile-array forms now preserve the percentile argument when
lowering `FILTER (WHERE ...)` to a null-preserving input expression. This keeps
the projection's aggregate name aligned with the aggregate state while rows
that fail the predicate contribute NULL to the KLL sketch.

Focused gate:

```text
$env:KAVEON_DIFF_ONLY='approx_percentile_filter'
cargo test -p kaveon-server --bin kaveon-server \
  differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
test result: ok. 1 passed; 0 failed
```

The case is:

```sql
SELECT APPROX_PERCENTILE(latency_p75_ms, 0.95)
  FILTER (WHERE actions > 5) AS p95
FROM events
```

The local evaluator and in-process three-worker fragment path agree across
dictionary, plain, multi-file, partitioned, and clustered Parquet layouts.
The test uses the existing percentile rank tolerance and verifies that the
query memory reservation is released. Scalar and constant percentile-array forms are qualified; dynamic percentile expressions and broader approximate aggregate semantics remain separate boundaries.


The same differential gate also covers APPROX_PERCENTILE(latency_p75_ms, ARRAY[0.5, 0.95]) FILTER (WHERE actions > 5) with exact array results across all layouts and three-worker execution.


Grouped constant percentile-array filters are also covered by the differential corpus, including one result per region with exact local and three-worker agreement.
