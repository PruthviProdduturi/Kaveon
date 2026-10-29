# `EXPLAIN ANALYZE` stage summaries — 2026-10-29

## What changed

`EXPLAIN ANALYZE` now retains the measured stage telemetry already collected by
the distributed executor. After the optimized plan and query-level timings,
the result includes one bounded line per observed stage with:

- stage state;
- completed tasks and task count;
- total task output rows and bytes; and
- stage wall-clock elapsed microseconds.

The same summary is emitted for the distributed aggregate path. Values are
observed execution counters, not estimates and not a second execution.

## Qualification

The server statement-API catalog test passed:

```text
cargo test -p kaveon-server api::tests::catalog_statements_run_through_the_statement_api
```

The repository formatting check passed with `cargo fmt --all -- --check`.

## Boundary

This closes the gap where stage timing and output volume were available in the
query record but absent from `EXPLAIN ANALYZE`. It does not claim per-operator
actuals: a stage can contain several operators, and operator-level counters
are still a separate qualification item. Estimates remain visible on physical
scan nodes through the plan-estimate qualification.
