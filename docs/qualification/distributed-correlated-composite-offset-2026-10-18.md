# Distributed correlated composite OFFSET qualification

Date: 2026-10-18

The distributed executor matches the local reference for a positive correlated
`OFFSET` when the subquery correlation is a composite equality key:

```sql
SELECT t.user_id, t.actions
FROM events t
WHERE EXISTS (
  SELECT *
  FROM events o
  WHERE o.user_id = t.user_id
    AND o.actions = t.actions
  OFFSET 1
)
ORDER BY t.user_id, t.actions
LIMIT 12;
```

The focused regression is
`distributed_correlated_exists_composite_offset_matches_local` in the server
differential suite. It compares local and three-worker distributed results and
asserts that the shared query reservation returns to zero.

The supported boundary is equality correlations. A non-equality residual such
as `o.actions > t.actions` remains intentionally unsupported because OFFSET
must be applied after that outer-row-dependent predicate; reducing it to a
grouped count by key would change SQL semantics.
