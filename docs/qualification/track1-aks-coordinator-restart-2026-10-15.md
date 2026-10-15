# Track 1 AKS coordinator restart qualification — 2026-10-15

This run restarts the coordinator while a distributed query is active. The
credential-safe raw report is retained locally as
`tmp/aks-coordinator-loss-2026-10-15.json`.

## Runtime

- Cluster: `kaveon-aks`, namespace `kaveon`
- Kubernetes: `1.35.7`
- Topology: one coordinator and three Engine workers
- Engine image: `kaveonacr.azurecr.io/kaveon-engine@sha256:c997febb6d14040bd9f26d21d1f96c90f33a6c5faf7a728cf2ba350e8181f06f`
- Active query: `SELECT COUNT(*) FROM kaveon_events_enriched WHERE actions >= 0`

## Results

All six checks passed:

- the query was observed in `RUNNING`;
- the in-flight request was interrupted when the coordinator restarted;
- the coordinator returned Ready;
- all three workers returned Ready;
- the post-restart exact probe returned `COUNT(*) = 24`; and
- coordinator exchange retention was zero before and after restart.

This qualifies fail-closed active-query interruption, startup cleanup, worker
reregistration, and exact post-restart recovery for the current topology.

## Boundary

This does not claim multi-coordinator failover or in-flight query continuity;
the interrupted query is intentionally not resumed.
