# Track 2: scalar subqueries inside scalar functions

Kaveon now lowers an uncorrelated scalar subquery when it appears as an
argument of a scalar function or inside a `CASE` expression. This covers the
common Trino shape where a one-row aggregate supplies a fallback value:

```sql
SELECT
  country,
  COALESCE((SELECT MAX(actions) FROM events), 0) AS max_actions
FROM events
GROUP BY country
ORDER BY country;
```

The lowering gives the scalar relation its existing single-row alias and
cross-joins it before normal expression binding. It does not introduce an
`Apply` operator or claim correlated scalar subqueries in arbitrary function
arguments; those still require the existing bounded correlation machinery.

The local planner and in-process three-worker fragment path return identical
canonical rows in
`distributed_scalar_subquery_inside_function_matches_local` in
`engine/crates/server/src/differential_tests.rs`. Planner coverage is in
`scalar_subqueries_inside_functions_are_lowered_before_binding` in
`engine/crates/sql/src/logical_plan.rs`.

This is a bounded expression-lowering improvement. Correlated scalar
subqueries nested inside functions, arbitrary table functions, and deeper
`Apply` shapes remain outside this qualification.
