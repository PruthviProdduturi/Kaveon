# Track 1: five-run distributed aggregate sample

## Workload

```sql
SELECT user_id, COUNT(*) AS rows
FROM OpenSource.public.kaveon_events_enriched
GROUP BY user_id
ORDER BY user_id
LIMIT 100
```

The query grouped the 504,000,000-row Delta source on the deployed AKS
three-worker topology. Five uncached submissions ran against the same catalog
snapshot and returned 100 rows each.

## Result

| Measure | Observation |
|---|---:|
| Exact result repeats | 5/5 identical SHA-256 result hashes |
| Wall times (seconds) | 23.047, 22.110, 22.515, 22.578, 22.094 |
| Mean wall time | 22.469 s |
| Nearest-rank p95 | 22.578 s |
| Stages / tasks | 3 / 7 per run |
| Workers | 3/3 on every run |
| Failures | 0 |
| Catalog snapshot | `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3` |

The raw report is `track1-aks-scale-aggregate-5x-2026-10-15.json`. The image
and worker identities are retained there for reproducibility.

## Interpretation

This strengthens distributed aggregate correctness and gives a larger repeated
latency sample for this exact workload. It is not a universal aggregate SLO:
there was no skewed-key corpus, no concurrent mixed workload in this run, and
it does not qualify ClickHouse or Trino performance parity.
