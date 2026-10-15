# Track 1: AKS object-store fault recovery

**Date:** 2026-10-15  
**Environment:** `kaveon-aks`, namespace `kaveon`  
**Worker fault target:** `kaveon-worker-0`  
**Query:** `SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0`

This qualification temporarily applied the `kaveon-storage-fault-injection`
NetworkPolicy to `kaveon-worker-0`. Cluster and DNS egress remained allowed;
public object-store traffic was blocked for the fault run. The policy was
removed in a `finally` path, and the report confirms that it was removed. No
subscription-level policy was changed and no credential material was recorded.

## Results

| Check | Result |
|---|---|
| Baseline result | `504000000` |
| Fault-injected result | `504000000` |
| Exact result digest | `17b44689d4c8fb0c665d8a42992b6e0ff9d15a1b3822afd043ad1a9d58b287c5` |
| Stage retry observed | Pass; fault run includes attempt `.0.0.1` |
| Retained exchange/spill files before | 0 |
| Retained exchange/spill files after | 0 |
| Temporary policy removed | Pass |

The fault run completed with the same single-row result as the baseline. The
retry attempt demonstrates that the stage recovered after the object-store
path was interrupted. Zero retained exchange and spill files before and after
the run provide the orphan-spool cleanup evidence for this scenario.

Raw evidence is recorded in
[`track1-aks-storage-fault-2026-10-15.json`](./track1-aks-storage-fault-2026-10-15.json).

## Boundary

This closes the explicit object-store fault and orphan-spool recovery check for
the tested AKS image and topology. It does not close the broader Track 1 gates:
sustained network/cloud-storage pressure, distributed join and aggregate
qualification, connector federation, upgrade/rollback, or the full mixed
workload soak.
