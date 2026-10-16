# Track 1 AKS: ten-round mixed workload endurance

## Deployment

- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:6a4fa35e1d511bdd533d6d3ee22c331137775a0848544bfd0fd98c2e6dea5b18`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Topology: one coordinator and three workers.

## Workload and result

Ten rounds submitted four queries concurrently: a grouped aggregate, a
filtered self-join, a partitioned `ROW_NUMBER()`/`QUALIFY`, and a global
`ORDER BY ... LIMIT`. The baseline for each shape was captured before the
rounds and every result was compared with that exact baseline.

| Measure | Observation |
|---|---:|
| Query executions | 40/40 finished |
| Exact results | 40/40 matched |
| Total round time | 604.001 s |
| Round p95 (nearest rank) | 64.032 s |
| Workers | 3/3 in the cluster |
| Retained worker spill files | 0 on every worker |
| Failures or restarts | 0 |

The raw per-query timings, resource samples, worker IDs, and cluster snapshot
are retained in `track1-aks-mixed-workload-10round-2026-10-16.json`.

## Boundary

This is a ten-round bounded endurance gate on one three-worker topology. It does
not qualify node loss during the soak, longer-duration endurance, cross-region
object stores, or a universal latency SLO.
