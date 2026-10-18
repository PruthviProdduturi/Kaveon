# Distributed `NOT IN` NULL-aware build qualification

Date: 2026-10-18

## Scope

This qualification covers a distributed anti join whose right side is a derived `VALUES` relation containing both a matching key and `NULL`, while the left side also contains a `NULL` probe. Trino SQL semantics require the result to be empty: a non-matching comparison is `UNKNOWN` when the subquery contains `NULL`, and a `NULL` probe is also `UNKNOWN`.

Query:

```sql
SELECT x.id
FROM (VALUES (1), (2), (NULL)) AS x(id)
WHERE x.id NOT IN (
  SELECT y.id FROM (VALUES (2), (NULL)) AS y(id)
)
ORDER BY x.id
```

## Result

The local planner and the three-worker fragment path both return zero rows, with the query memory pool returning to zero after each run. The regression is `distributed_not_in_null_subquery_matches_trino_three_valued_semantics` in `engine/crates/server/src/differential_tests.rs`.

The distributed stage graph broadcasts an anti-join build side so every worker observes the complete right-side key set. A hash exchange would route a `NULL` key to only one partition and could incorrectly emit a non-matching row from another worker. Ordinary semi joins retain the distributed hash exchange.

## Boundary

This qualifies the null-aware behavior for the distributed anti-join path and derived `VALUES` input. It does not claim a general cost-based choice between broadcast and hash anti joins; large anti-join builds need a null-aware exchange or statistics-driven strategy before broad performance qualification.
