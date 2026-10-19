# Track 2: aggregate `FILTER (WHERE ...)`

Kaveon supports the Trino aggregate-filter form for the supported exact
aggregates. The planner lowers the filter to a NULL-preserving input
expression: rows that fail the predicate contribute NULL, while `COUNT(*)`
uses a non-NULL constant for rows that pass. This lets the existing local,
partitioned, spill, and partial-state implementations retain one merge
contract instead of adding a second aggregate state format.

The qualification corpus covers the basic grouped form:

```sql
SELECT region,
       COUNT(*) FILTER (WHERE sessions > 0) AS active_n,
       SUM(actions) FILTER (WHERE actions > 0) AS positive_actions
FROM events
GROUP BY region
ORDER BY region
```

The same differential run also covers NULL predicates, `COUNT(DISTINCT ...)`,
`AVG`, `MIN`, and `MAX` in one grouped statement:

```sql
SELECT region,
       COUNT(*) FILTER (WHERE industry IS NULL) AS null_industry,
       COUNT(DISTINCT country) FILTER (WHERE actions > 20) AS active_countries,
       AVG(CAST(actions AS DOUBLE)) FILTER (WHERE actions > 20) AS active_avg,
       MIN(actions) FILTER (WHERE actions > 20) AS active_min,
       MAX(actions) FILTER (WHERE actions > 20) AS active_max
FROM events
GROUP BY region
ORDER BY region
```

`the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`
compares both statements across dictionary and plain Parquet, multi-file,
partitioned, clustered, and the in-process three-worker fragment path. The
focused regression is selected with:

```powershell
$env:KAVEON_DIFF_ONLY = 'filtered_aggregate_null_distinct_and_extrema'
cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths -- --exact --nocapture
```

The focused run passed on 2026-10-18 with exact local/distributed equality and
zero query-memory leakage. Window aggregate filters use the same lowering and
have a separate corpus case. `COLUMN_STATISTICS(...) FILTER (...)` remains
explicitly rejected because its profile semantics include NULL accounting and
cannot safely inherit the ordinary aggregate lowering.

