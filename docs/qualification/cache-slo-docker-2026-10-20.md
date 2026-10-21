# Docker result-cache qualification — 2026-10-20

The coordinator result cache was qualified against the running Docker topology with two Engine workers.

## Workload and command

```sql
SELECT country, COUNT(*) AS n
FROM OpenSource.public.covid_global
GROUP BY country
ORDER BY country
```

The harness ran an uncached baseline, a cold cache fill, ten warm hits, a cache-bypass query, an administrative clear, and a separate coordinator restart followed by an identical query.

## Result

The cache gate passed all checks:

- exact result parity across uncached, cold, warm, and bypass results;
- 10/10 warm hits;
- bypass did not add a hit;
- administrative clear removed entries and bytes;
- hit-latency counters were exposed;
- coordinator restart produced a miss before repopulating the cache.

Warm-hit latency was p50 **11.983 ms**, p95 **15.336 ms**, and max **15.336 ms**. The restart evidence recorded one miss with zero hits after the restart. This qualifies the local Docker cache lifecycle; cross-coordinator invalidation and cloud cache endurance remain separate gates.

