# Track 1 AKS node-drain retry qualification — 2026-10-15

This run uses graceful node drain while a grouped, skew-sensitive query is
active. The credential-safe raw report is retained locally as
`tmp/aks-skew-node-drain-2026-10-15.json`.

## Runtime

- Cluster: `kaveon-aks`, namespace `kaveon`
- Kubernetes: `1.35.7`
- Topology: three Engine workers
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Failure mode: graceful node drain of one worker
- Query: `SELECT region, COUNT(*), SUM(actions) FROM kaveon_events_enriched GROUP BY region ORDER BY region`

## Results

The harness passed all seven checks: catalog-compatible workers before the run,
concurrent exact results, cancellation cleanup, worker-loss retry and
replacement, bounded pressure without unexpected restarts or retained files,
no pre-existing exchange or spill files, and an exact post-recovery probe.

The run observed graceful loss of `kaveon-worker-1`, replacement scheduling,
and an actual retry attempt. The six grouped result rows matched the known
exact result, including the 504,000,000-row total. All three workers returned
Ready after recovery.

This closes the node-drain retry gate for the declared three-worker topology.
It does not claim coordinator failover, multi-coordinator continuity, or a
multi-hour mixed-workload soak.
