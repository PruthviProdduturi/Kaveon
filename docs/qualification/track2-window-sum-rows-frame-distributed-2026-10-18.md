# Distributed partitioned window `SUM` ROWS-frame qualification

Date: 2026-10-18

## Scope

This qualification covers a partitioned aggregate window with a bounded `ROWS BETWEEN 1 PRECEDING AND CURRENT ROW` frame. The ordering includes a deterministic key so each worker can reproduce the same row boundaries.

```sql
SELECT country,
       region,
       user_id,
       actions,
       SUM(actions) OVER (
         PARTITION BY region
         ORDER BY actions, user_id
         ROWS BETWEEN 1 PRECEDING AND CURRENT ROW
       ) AS rolling_actions
FROM events
WHERE event_date = '2026-07-04'
ORDER BY region, actions, user_id
LIMIT 18
```

## Result

The local planner and three-worker fragment path return the same 18 exact rows. The query memory pool returns to zero after each execution. The regression is `distributed_partitioned_window_sum_rows_frame_matches_local` in `engine/crates/server/src/differential_tests.rs`.

## Boundary

This qualifies a bounded aggregate `ROWS` frame through local and in-process distributed execution. It does not claim all frame modes, RANGE/GROUPS peer semantics, unbounded window workloads, external-client wire qualification, or cloud qualification.
