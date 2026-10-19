# Track 2 CTE shadowing and recursion guard — 2026-10-18

## Scope

This qualification covers two planner safety boundaries for Trino-compatible
common table expressions:

- a non-recursive CTE that refers to itself fails with a SQL error while it is
  being lowered; it does not recurse until the planner stack overflows;
- an inner `WITH` definition may shadow an outer CTE with the same name and is
  expanded as its own definition.

The guard uses a stable identity for each CTE definition rather than only its
name. This matters because a nested scope can reuse a name without being
recursive.

## Exact statements

The SQL planner regression is:

```sql
WITH rows AS (SELECT * FROM rows) SELECT * FROM rows
```

It fails closed with:

```text
recursive reference to non-recursive CTE 'rows'
```

The local and three-worker differential is:

```sql
WITH rows AS (
  WITH rows AS (VALUES (1), (2))
  SELECT column1 FROM rows
)
SELECT column1 FROM rows
ORDER BY column1
```

Both execution paths return the exact rows `1` and `2` in order, and the
query-memory pool returns to zero bytes after execution.

## Validation

Run from `engine/`:

```powershell
cargo test -p kaveon-sql cte_ --quiet
cargo test -p kaveon-server distributed_nested_cte_shadowing_matches_local_semantics --quiet
```

Observed result on the integration worktree: 12 SQL CTE tests passed and the
three-worker differential test passed.

This does not expand recursive CTE support. General recursive graphs,
multiple recursive members, recursive joins, and multi-column recursion remain
outside the declared Track 2 contract.
