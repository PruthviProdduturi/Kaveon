# AKS bounded high-cardinality join qualification — 2026-10-15

The current three-worker AKS Engine completed an exact-repeat self-join over
`OpenSource.public.kaveon_events_enriched`, the 504M-row table. The bounded
probe used a seven-day slice and `user_id <= 100000` on both inputs to exercise
real distributed repartitioning without an unbounded join result.

```sql
SELECT a.region, COUNT(*) AS rows
FROM kaveon_events_enriched a
JOIN kaveon_events_enriched b ON a.user_id = b.user_id
WHERE a.user_id <= 100000 AND b.user_id <= 100000
  AND a.event_date >= DATE '2026-07-04'
  AND a.event_date <= DATE '2026-07-10'
  AND b.event_date >= DATE '2026-07-04'
  AND b.event_date <= DATE '2026-07-10'
GROUP BY a.region ORDER BY a.region
```

Two runs finished with the same six-row result hash
`13718a6e2e26649d6e124fc2b5deb3f2bcb52500d7a4506cf999caaf1235cf31`, using five
stages, 13 tasks, and all three workers. Wall times were 16.672s and 16.578s.
Both runs pinned catalog snapshot
`sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`.

Raw machine-readable evidence is retained in
`track1-aks-high-cardinality-join-2026-10-15.json`. This qualifies the bounded
high-cardinality shape. Larger input volumes, disk-exhaustion behavior, and
sustained mixed-workload endurance remain open.
