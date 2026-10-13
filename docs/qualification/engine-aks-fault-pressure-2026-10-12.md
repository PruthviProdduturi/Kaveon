# AKS fault, recovery, and pressure qualification — 2026-10-12

This is a deployed Engine qualification run on `kaveon-aks` in namespace
`kaveon`. The full machine-readable report is stored beside this note as
`engine-aks-fault-pressure-2026-10-12.json`.

| Field | Result |
| --- | --- |
| Engine image | `sha256:60125b538234ceb6e5f54695e0fbeab8926dfdd52f79a969afb81768d891e665` |
| Workers before | 3, all catalog-compatible |
| Concurrent exact queries | 9 (4-way concurrency × 3 rounds) |
| Pressure requests | 12 (4-way concurrency × 3 rounds) |
| Fault workload | `SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0` |
| Fault input | 504,000,000 rows |
| Worker loss | Forced termination during a running distributed scan |
| Recovery | Replacement worker returned catalog-compatible; retry attempt observed |
| Exact recovery | Passed; result remained 504,000,000 |
| Query/result failures | 0 |
| Pre-existing spill/exchange files | 0 |
| Post-recovery exact probe | Passed |

All report checks passed: three-worker compatibility, concurrent exact results,
worker-loss retry and recovery, bounded pressure without unexpected restart or
file growth, no pre-existing retained exchange/spill files, and post-recovery
exactness. This closes the deployed worker-loss and bounded-pressure gate for
the current image. It does not close the remaining Track 1 gates for node
drain, coordinator restart, sustained soak, parallel global ordering, or
cloud-storage failure injection.
