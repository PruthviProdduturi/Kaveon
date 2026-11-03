# Exact distinct context answers — 2026-11-03

## Contract

Kaveon may answer an unfiltered, ungrouped `COUNT(DISTINCT column)` directly
from source-version-bound statistics when the table has been explicitly
profiled with:

```sql
ANALYZE catalog.schema.table WITH (distinct = true);
```

The answer is exact for the pinned source version and excludes null values,
matching the live aggregate. Missing or stale `distinct_exact` metadata, a
predicate, grouping, join, derived source, or any other unsupported shape stays
on the scan path. The shortcut is therefore fail-closed: statistics are never
used to answer a changed source.

## Evidence

The API regression
`context_answers_equal_the_scanned_answers_and_refuse_a_changed_source`
passes on the current Engine tree. It verifies that:

1. the same query scans before exact distinct profiling;
2. `ANALYZE ... WITH (distinct = true)` persists the exact count;
3. `SELECT COUNT(DISTINCT id) FROM events` returns the scanned value from
   `execution.mode: "context"` with no scan records; and
4. changing the source version forces the query back to a scan.

The existing cube/statistics regression
`cube_answers_equal_the_scanned_answers_refuse_a_changed_source_and_refresh`
also passes, preserving the broader context-answer boundary.

Validation run on 2026-11-03:

```text
cargo test -p kaveon-server --bin kaveon-server \
  context_answers_equal_the_scanned_answers_and_refuse_a_changed_source -- --nocapture
  1 passed

cargo test -p kaveon-server --bin kaveon-server \
  cube_answers_equal_the_scanned_answers_refuse_a_changed_source_and_refresh -- --nocapture
  1 passed

cargo fmt --all -- --check
cargo clippy -p kaveon-server --bin kaveon-server -- -D warnings
  passed
```

This qualification covers the exact single-table total only. Grouped distinct
counts, filtered distinct counts, joins, and approximate distinct sketches
retain their existing contracts and evidence.
