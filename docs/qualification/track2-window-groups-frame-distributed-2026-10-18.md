# Track 2 qualification: distributed window GROUPS frame

Date: 2026-10-18  
Surface: partitioned window aggregation  
Status: qualified by local and three-worker differential regression

## Scope

The distributed window path preserves peer-group boundaries for a bounded
`GROUPS` frame. The regression
`distributed_partitioned_window_sum_groups_frame_matches_local` in
`engine/crates/server/src/differential_tests.rs` compares local execution with
three in-process workers for:

```sql
SELECT country, region, user_id, actions,
       SUM(actions) OVER (
         PARTITION BY region
         ORDER BY actions
         GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW
       ) AS grouped_actions
FROM events
WHERE event_date = '2026-07-04'
ORDER BY region, actions, user_id
LIMIT 18
```

The exact result and memory release match between local and distributed
execution.

## Boundary

This qualifies a bounded partitioned `SUM` with a `GROUPS` frame. Other GROUPS
functions, unbounded workloads, peer-order ties without a deterministic final
ordering, external client wire coverage, and cloud qualification remain open.
