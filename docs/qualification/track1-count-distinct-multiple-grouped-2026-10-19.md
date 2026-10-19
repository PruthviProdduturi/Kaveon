# Track 1 — grouped multiple exact `COUNT(DISTINCT)` aggregates

The distributed differential corpus now includes two exact distinct aggregates
in one grouped plan:

```sql
SELECT region,
       COUNT(DISTINCT country) AS countries,
       COUNT(DISTINCT surface) AS surfaces
FROM events
GROUP BY region
ORDER BY region
```

The case is compared across the local evaluator, dictionary and plain Parquet,
multi-file, partitioned, clustered, and in-process three-worker fragment paths.
Results are canonicalized before comparison, so duplicate values are unioned per
aggregate state rather than adding per-worker cardinalities. NULL inputs remain
excluded independently for each distinct expression.

Evidence from `engine/`:

```text
cargo fmt --all -- --check
cargo test -p kaveon-server the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --nocapture
test ... has been running for over 60 seconds
test differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths ... ok
test result: ok; 1 passed; 0 failed; 610 filtered out; finished in 449.40s
```

This qualifies the exact multi-distinct grouped shape on the local and
in-process three-worker topology. It does not close the larger-scale skew,
physical-disk exhaustion, deployed cloud endurance, or external pinned-Trino
differential boundaries.
