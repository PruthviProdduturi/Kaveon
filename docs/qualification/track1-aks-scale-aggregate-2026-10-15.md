# AKS full-table aggregate qualification — 2026-10-15

The current three-worker AKS Engine executed a full-table grouped aggregate over
`OpenSource.public.kaveon_events_enriched` (504,000,000 input rows):

```sql
SELECT user_id, COUNT(*) AS rows
FROM kaveon_events_enriched
GROUP BY user_id
ORDER BY user_id
LIMIT 100
```

The `LIMIT` bounds result serialization; the grouped aggregate still processes
the full source. Two runs finished with the identical 100-row result hash
`9e6816aae279642b5d01582a250a7be0dbc7ad3827354ec8d1efb8073dd62c0d`. Each used
three stages, seven tasks, and all three workers. Wall times were 22.640s and
23.891s. The raw report is retained in
`track1-aks-scale-aggregate-2026-10-15.json`.

This provides full-table distributed aggregate correctness and repeatability
evidence. A larger repeated sample is still required before claiming a p95 SLO,
and mixed-workload, skew, disk-exhaustion, and final-merge spill qualification
remain open.
