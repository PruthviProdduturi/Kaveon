# Track 2 AKS: current-image SQL wire corpus

## Result

The current Engine image passed all eleven Trino-shaped SQL cases twice with
exactly identical results on each repeat. Every request finished successfully
with three catalog-compatible workers and one pinned catalog snapshot.

The corpus covers non-recursive CTEs, aggregate `FILTER`, `GROUPING SETS`,
window frames, `FETCH ... WITH TIES`, scalar subqueries, array/map/JSON
expressions, `UNNEST ... WITH ORDINALITY`, constant `CROSS JOIN LATERAL
UNNEST`, `EXISTS`, and `UNION ALL`.

The raw report is retained in
`track2-aks-wire-corpus-lateral-current-2026-10-16.json`. It records the live
cluster metadata, including the pinned catalog snapshot and three workers.

## Boundary

This is a compatibility corpus for the declared shapes. Correlated/lateral
table functions, recursive CTEs, broader correlated Apply forms, complete JSON
coercion/path semantics, and general DML remain explicitly partial in the
parity tracker.
