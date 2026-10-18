# Distributed correlated lateral ordered LIMIT qualification

Date: 2026-10-18

Kaveon preserves local semantics for a bounded correlated lateral relation
that selects the per-key minimum through `ORDER BY` and `LIMIT 1`:

```sql
SELECT t.user_id, x.actions
FROM events t
CROSS JOIN LATERAL (
  SELECT o.actions
  FROM events o
  WHERE o.user_id = t.user_id
  ORDER BY o.actions
  LIMIT 1
) x
ORDER BY t.user_id, x.actions
LIMIT 12;
```

The regression `distributed_correlated_lateral_ordered_limit_matches_local`
compares local and three-worker distributed results and verifies zero retained
query memory. Broader correlated lateral paging and multi-column scalar row
subqueries remain outside this qualification.
