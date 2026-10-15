# Track 1 AKS skewed-aggregation fault qualification — 2026-10-15

This qualification combines a deliberately grouped, skew-sensitive workload
with forced worker loss. The raw credential-safe report is retained locally as
`tmp/aks-skew-fault-2026-10-15.json`.

## Runtime

- Cluster: `kaveon-aks`, namespace `kaveon`
- Kubernetes: `1.35.7`
- Topology: three Engine workers
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Fault mode: forced deletion of one worker while the grouped query was running
- Query: `SELECT region, COUNT(*), SUM(actions) FROM kaveon_events_enriched GROUP BY region ORDER BY region`

## Results

All harness checks passed: concurrent exact results, cancellation cleanup,
worker-loss retry and replacement, bounded pressure without unexpected
restarts or retained files, and the exact post-recovery probe.

The six grouped rows were identical before and during worker loss:

| Region | Rows | Actions |
|---|---:|---:|
| Africa | 26,287,632 | 440,629,155 |
| Asia | 226,608,984 | 3,796,204,816 |
| Europe | 105,767,760 | 1,773,331,824 |
| North America | 110,193,384 | 1,846,794,099 |
| Oceania | 7,734,216 | 129,208,527 |
| South America | 27,408,024 | 460,478,299 |

The harness observed loss of `kaveon-worker-1`, replacement scheduling, and a
retry attempt with an exact result match. This closes the declared live
skew/failure-injection evidence gate for the current three-worker image.

## Boundary

This is evidence for grouped execution under worker loss. It does not claim a
general skew-splitting optimizer, coordinator failover, node-drain lease retry,
or sustained multi-hour soak. The separate node-drain attempt remains negative
evidence because graceful eviction allowed the task to finish without an
observed retry.
