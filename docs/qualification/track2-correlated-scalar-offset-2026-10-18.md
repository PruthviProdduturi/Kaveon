# Track 2 qualification: correlated scalar `OFFSET … LIMIT 1`

Date: 2026-10-18

## Contract

Kaveon supports a bounded scalar subquery with equality correlations, a
positive `OFFSET`, and `LIMIT 1`. The binder decorrelates the subquery by
partitioning its rows by the equality keys, assigning `ROW_NUMBER()` in the
requested order, and selecting the requested row. Residual correlations,
multiple rows, offset-only forms, and general Apply shapes remain rejected.

```sql
SELECT COUNT(*) AS n
FROM events e
WHERE e.actions > (
  SELECT i.actions
  FROM events i
  WHERE i.user_id = e.user_id
  ORDER BY i.actions
  OFFSET 1 LIMIT 1
)
```

## Focused evidence

Run from `engine/`:

```powershell
cargo test -p kaveon-optim --quiet
$env:KAVEON_DIFF_ONLY = "correlated_scalar_ordered_offset_limit"
cargo test -p kaveon-server --bin kaveon-server `
  the_differential_sweep_matches_across_parquet_encodings_and_execution_paths --quiet
```

The focused differential case passed with identical local and three-worker
results. The optimizer tests also assert the per-key row-number lowering and
the explicit residual-correlation rejection.

## Scope

This evidence covers only equality-correlated scalar `OFFSET n LIMIT 1`.
Positive correlated offsets with residual predicates, multi-column scalar
limits, unbounded or offset-only scalar relations, and broader lateral/Apply
forms remain outside the qualified surface.
