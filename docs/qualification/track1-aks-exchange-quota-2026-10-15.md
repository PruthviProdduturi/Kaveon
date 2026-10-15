# Track 1 AKS exchange quota qualification — 2026-10-15

This is a controlled live quota probe against the three-worker release image.
The credential-safe probe driver is retained locally as
`tmp/aks-quota-probe.py`; no credential material is written to the repository.

## Setup

The coordinator and workers were rolled temporarily with:

- exchange process limit: 1 MiB;
- per-query exchange limit: 512 KiB.

The original limits were restored after the probe and both StatefulSets reached
Ready: coordinator 24 GiB / 16 GiB per query, workers 8 GiB / 6 GiB per query.

## Probe

The query was:

```sql
SELECT user_id, COUNT(*)
FROM kaveon_events_enriched
GROUP BY user_id
```

It produced a high-cardinality distributed exchange and was rejected with HTTP
507. The returned error identified `exchange endpoint returned HTTP 507` for
the exhausted exchange. This is the expected fail-closed quota behavior.

Exchange file counts were zero before and after the run on the coordinator and
all workers. The normal grouped workload was also rerun under the same low
quota before this high-cardinality probe and completed exactly, confirming the
quota boundary is exercised by exchange volume rather than a blanket query
failure.

## Boundary

This qualifies live exchange quota rejection and cleanup on the current AKS
image. Object-store network fault injection and orphan-spool recovery are
qualified separately in `track1-aks-storage-fault-2026-10-15.md`. Longer
mixed-workload endurance remains open.
