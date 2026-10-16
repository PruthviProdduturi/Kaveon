# AKS five-run aggregate p95 qualification — 2026-10-15

The current three-worker Engine repeated the full-table grouped aggregate five
times. The query grouped all 504,000,000 rows in
`OpenSource.public.kaveon_events_enriched` and returned only the first 100
ordered groups for bounded result serialization:

```sql
SELECT user_id, COUNT(*) AS rows
FROM kaveon_events_enriched
GROUP BY user_id
ORDER BY user_id
LIMIT 100
```

All five requests finished with the identical result hash
`9e6816aae279642b5d01582a250a7be0dbc7ad3827354ec8d1efb8073dd62c0d`, using
three stages, seven tasks, and all three workers. Wall times were
24.546s, 24.391s, 23.422s, 23.594s, and 22.843s; the observed p95 for this
five-run sample is **24.546s**. The raw report is retained in
`track1-aks-scale-aggregate-5x-2026-10-15.json`.

This is measured aggregate p95 evidence for the declared workload, not a
universal SLO. Skew, disk-exhaustion, final-merge spill under sustained mixed
load, and a larger statistical sample remain open.
