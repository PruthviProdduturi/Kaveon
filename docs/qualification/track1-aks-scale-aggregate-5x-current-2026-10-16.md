# Track 1 AKS: current-image distributed aggregate sample

## Result

The immutable Engine image `sha256:60de4cf526c2340959fcd0ce264c9c4cca1680ced8d462617c4287d93e01f8c9` ran the full grouped aggregate over the 504,000,000-row `OpenSource.public.kaveon_events_enriched` table five times on the three-worker AKS topology.

Every run finished successfully with three stages, seven tasks, all three workers, 100 exact result rows, and the same result hash (`9e6816aae279642b5d01582a250a7be0dbc7ad3827354ec8d1efb8073dd62c0d`). Wall times were 24.157s, 23.578s, 23.031s, 26.453s, and 24.016s; the observed five-sample p95 is 26.453s. No failure or restart was observed by the qualification client.

The raw report is retained in
`track1-aks-scale-aggregate-5x-current-2026-10-16.json`.

## Boundary

This is a bounded correctness and workload sample, not a universal latency SLO. Skew behavior, sustained mixed-workload pressure, physical disk exhaustion, larger topologies, and broader aggregate-function coverage remain open in the Track 1 tracker.