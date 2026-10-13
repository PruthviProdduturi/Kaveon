# AKS result-cache qualification — 13 October 2026

The deployed coordinator image was qualified through the fail-closed cache
harness against the row-result workload below.

```sql
SELECT * FROM OpenSource.public.kaveon_events_dashboard LIMIT 10
```

Image: `sha256:9c0821abf24cf5646de44c0009a8f83b85f752933928e574ce7e8d4741b60f3d`  
Topology: `kaveon-aks`, namespace `kaveon`, three workers.

The run passed exact-result parity, 20/20 warm hits, bypass isolation, cache
clear, authorization-scope isolation, and coordinator restart-to-miss. Warm-hit
latency was p50 15.577 ms, p95 21.740 ms, p99 81.914 ms, and max 81.914 ms;
the Engine reported hit-latency samples and nanosecond counters.

This evidence is scoped to the declared analytical row-result workload. It does
not qualify shared cross-coordinator invalidation, TTL-expiry behavior,
cache-backed transactional reads, or the full distributed-execution SLO.
The machine-readable record is adjacent to this file.
