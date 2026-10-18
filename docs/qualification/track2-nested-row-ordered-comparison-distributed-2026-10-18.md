# Distributed nested ROW ordered comparison qualification

Date: 2026-10-18

## Scope

This qualification covers lexicographic ordering through nested ROW fields and verifies SQL three-valued NULL propagation when an inner ROW field is NULL.

```sql
SELECT ROW(ROW(1, 2), 3) < ROW(ROW(1, 3), 3) AS nested_less,
       ROW(ROW(2, CAST(NULL AS BIGINT)), 0)
         > ROW(ROW(1, 99), 0) AS nested_later,
       ROW(ROW(1, CAST(NULL AS BIGINT)), 3)
         < ROW(ROW(1, 3), 3) AS nested_unknown
FROM (VALUES (1), (2), (3)) AS input(id)
ORDER BY input.id
```

## Result

The local planner and three-worker fragment path return three exact `true | true | NULL` rows. The query memory pool returns to zero after both executions. The regression is `distributed_nested_row_ordered_comparison_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies nested ROW lexicographic ordering and NULL propagation through local and in-process distributed execution. It does not claim MAP ordering, unsupported non-orderable complex types, external-client wire qualification, or cloud qualification.
