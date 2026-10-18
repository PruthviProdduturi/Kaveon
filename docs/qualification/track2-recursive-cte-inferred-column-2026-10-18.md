# Track 2 qualification: bounded recursive CTE with inferred column

Date: 2026-10-18

## Contract

Kaveon accepts the bounded, single-column Trino recursive form where the CTE
column list is omitted and the seed names its one column explicitly:

```sql
WITH RECURSIVE numbers AS (
  SELECT 1 AS n
  UNION ALL
  SELECT n + 1 FROM numbers WHERE n < 5
)
SELECT n FROM numbers ORDER BY n
```

The parser infers `n` from the seed and lowers the statement to the existing
`SEQUENCE`/`UNNEST` path. This remains deliberately bounded: a nameless seed,
multiple seed columns, multiple recursive members, recursive joins, and
general recursive graphs are still rejected rather than being given partial
semantics.

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-sql --quiet
cargo test -p kaveon-server --bin kaveon-server differential_tests::the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

The SQL package includes parser and logical-plan assertions for the inferred
column and for the nameless-seed rejection. The server differential corpus
includes `bounded_recursive_cte_inferred_column`, so the same exact result is
checked through local and three-worker partitioned execution across the corpus
storage layouts.

## Scope

This evidence covers one bounded inferred-column shape only. It does not claim
general recursive CTE support, recursive joins, recursive graph traversal, or
external-client wire qualification.
