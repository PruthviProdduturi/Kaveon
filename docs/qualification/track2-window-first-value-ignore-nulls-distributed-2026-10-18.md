# Distributed unpartitioned `FIRST_VALUE IGNORE NULLS` qualification

Date: 2026-10-18

## Scope

This qualification covers an unpartitioned value window with `FIRST_VALUE(... IGNORE NULLS)`. The global ordering includes `user_id` as a deterministic tie breaker, and the result is compared between local execution and three distributed workers.

```sql
SELECT country,
       user_id,
       actions,
       FIRST_VALUE(industry) IGNORE NULLS OVER (
         ORDER BY actions, user_id
       ) AS first_industry
FROM events
WHERE event_date = '2026-07-04'
ORDER BY actions, user_id
LIMIT 18
```

## Result

Both paths return the same 18 exact rows, and the query memory pool returns to zero after each execution. The regression is `distributed_unpartitioned_first_value_ignore_nulls_matches_local` in `engine/crates/server/src/differential_tests.rs`.

The focused case verifies that NULL skipping and the global window order survive the distributed merge instead of being evaluated independently per worker fragment.

## Boundary

This qualifies unpartitioned `FIRST_VALUE IGNORE NULLS` through local and in-process distributed execution. It does not claim all value-window modifiers, nondeterministic peer ordering, external-client wire qualification, or cloud qualification.
