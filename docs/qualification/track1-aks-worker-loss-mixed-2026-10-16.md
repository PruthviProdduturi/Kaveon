# Track 1 AKS: worker loss during mixed pressure

## Deployment

- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:6a4fa35e1d511bdd533d6d3ee22c331137775a0848544bfd0fd98c2e6dea5b18`
- Catalog snapshot: `sha256:652742525737d79eb579cdef4a8386b6fa6c9f3feedc60b197bd3406ab73d1f3`
- Topology: one coordinator and three workers.

## Fault gate

The run captured exact baselines, concurrent exact requests, active-query
cancellation, then force-terminated `kaveon-worker-1` while a distributed
504M-row scan was running. The StatefulSet replacement returned with a new UID
and the same catalog snapshot. The fault query retried and returned the exact
baseline result.

| Check | Result |
|---|---:|
| Catalog-compatible workers before | 3/3 |
| Worker-loss retry observed | yes |
| Fault result exact match | yes |
| Replacement worker | new UID, Ready |
| Concurrent exact pressure | 8/8 |
| Cancellation cleanup | passed |
| Retained exchange/spill files | 0 before and after |
| Unexpected restarts | 0 |
| Post-recovery exact probe | passed |

The complete query records, worker IDs, resource samples, and cluster snapshot
are retained in `track1-aks-worker-loss-mixed-2026-10-16.json`.

## Boundary

This qualifies one forced worker loss during a bounded mixed-pressure run on
one three-worker AKS topology. Repeated losses, node loss during a long soak,
physical disk exhaustion, and broader topology qualification remain open.
