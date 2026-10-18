# Distributed partitioned `LAG IGNORE NULLS` qualification

Date: 2026-10-18

## Scope

This qualification covers a partitioned value window with `LAG(... IGNORE NULLS)`, an explicit deterministic tie breaker, and a bounded result. The local planner and three-worker fragment path must skip NULL predecessor values within each country partition while preserving the ordered rows.

```sql
SELECT country,
       user_id,
       actions,
       LAG(industry) IGNORE NULLS OVER (
         PARTITION BY country
         ORDER BY actions, industry NULLS LAST, user_id
       ) AS previous_industry
FROM events
WHERE event_date = '2026-07-04'
ORDER BY country, actions, industry NULLS LAST, user_id
LIMIT 12
```

## Result

Local and distributed execution return the same 12 exact rows, and the query memory pool returns to zero after each run. The regression is `distributed_partitioned_lag_ignore_nulls_matches_local` in `engine/crates/server/src/differential_tests.rs`.

The explicit `user_id` tie breaker is part of the qualification: value-window results are otherwise order-sensitive when peer rows share every declared ordering key.

## Boundary

This qualifies partitioned `LAG IGNORE NULLS` through local and in-process distributed execution. It does not claim full window modifier coverage, nondeterministic peer ordering, external-client wire qualification, or cloud qualification.
