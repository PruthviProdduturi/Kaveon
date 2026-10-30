# Track 2 — T-SQL bracket identifiers in distinct projections

## Scope

SQL Server bracket-quoted identifiers such as `[year]` must remain column
identifiers when they appear in a projection, `DISTINCT`, `ORDER BY`, or
`LIMIT`/pagination shape. They must not be rewritten as an `ARRAY` expression
or otherwise change the expression tree.

## Evidence

The distributed differential sweep includes the exact regression:

```sql
SELECT DISTINCT [region] FROM events ORDER BY [region]
SELECT DISTINCT [actions] FROM events ORDER BY [actions] LIMIT 5
```

The cases are evaluated against dictionary-encoded, plain, partitioned,
clustered, and fragmented Parquet layouts and through three-worker execution.
The local and distributed canonical rows match for both cases. The focused
gate is:

```text
KAVEON_DIFF_ONLY=distinct_bracketed_identifier \
  cargo test -p kaveon-server --bin kaveon-server \
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths
```

Result on the 2026-10-29 Engine tree: `1 passed`.

The parser unit suite also verifies that numeric/list brackets remain array
syntax while identifier brackets are quoted, so `ARRAY[1, 2]` is not confused
with `[year]`.

## Boundary

This qualifies bracket-quoted identifiers in the tested projection and order
shapes. It does not claim general SQL Server identifier compatibility for
unqualified objects, dynamic SQL, or unsupported table-valued syntax.
