# Distributed partitioned `QUALIFY ROW_NUMBER` qualification

Date: 2026-10-18

## Scope

This qualification covers the Trino-style `QUALIFY` modifier filtering a partitioned `ROW_NUMBER()` after window evaluation. The ranking uses a deterministic peer tie breaker and is executed against the local planner and three-worker fragment path.

```sql
SELECT country,
       region,
       actions,
       ROW_NUMBER() OVER (
         PARTITION BY region
         ORDER BY actions DESC, country, user_id
       ) AS rn
FROM events
WHERE event_date = '2026-07-04'
QUALIFY rn <= 2
ORDER BY region, rn, country
```

## Result

Local and distributed execution return the same non-empty exact result set. The query memory pool returns to zero after both executions. The regression is `distributed_partitioned_qualify_row_number_matches_local` in `engine/crates/server/src/differential_tests.rs`.

The test verifies that `QUALIFY` is applied after window ranking and after the distributed window result is combined, rather than being pushed into the input scan.

## Boundary

This qualifies partitioned `ROW_NUMBER` with a bounded `QUALIFY` predicate through local and in-process distributed execution. It does not claim arbitrary window expressions, unsupported QUALIFY predicates, external-client wire qualification, or cloud qualification.
