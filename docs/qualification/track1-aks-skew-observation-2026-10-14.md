# Track 1 AKS skew observation — 2026-10-14

**Cluster:** `kaveon-aks` / namespace `kaveon`  
**Query ID:** `922a2edf-7989-4716-af5a-8d7972679afe`  
**Image:** `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`

The current three-worker AKS image ran this exact distributed grouped scan with
statistics and result-cache shortcuts disabled:

```sql
SELECT region, COUNT(*) AS n
FROM public.kaveon_events_enriched
GROUP BY region
ORDER BY n DESC
```

The statement finished in `34,923 ms` with six exact rows:

| Region | Rows |
|---|---:|
| Asia | 226,608,984 |
| North America | 110,193,384 |
| Europe | 105,767,760 |
| South America | 27,408,024 |
| Africa | 26,287,632 |
| Oceania | 7,734,216 |

The largest group is 29.3× the smallest, so the input is materially skewed.
Stage 0 assigned one task to each worker; task elapsed times were 26.817 s,
34.152 s, and 28.724 s (1.27× max/min). Stages 1 and 2 completed the exchange
and final merge on the same snapshot, with no error or partial result.

This is live skew and exact-result evidence for the current image. It does not
claim that a general skew-splitting optimizer exists; that design and its
qualification remain open in the parity tracker.
