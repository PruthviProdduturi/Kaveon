# Track 1 AKS: current-image global ordering and TopN qualification

## Result

The immutable Engine image `sha256:60de4cf526c2340959fcd0ce264c9c4cca1680ced8d462617c4287d93e01f8c9` ran a global multi-column ordering query with `NULLS LAST`, `OFFSET 100`, and `LIMIT 20` twice on the three-worker AKS topology.

Both runs finished successfully with two stages, four tasks, all three workers, and the same exact 20-row result hash (`fc0909d42a14f1c26efff9555f60331c3112d531c410eb0780fdb69bb63b1cfd`).

The raw report is retained in
`track1-aks-global-order-current-2026-10-16.json`.

## Boundary

This qualifies exact global ordering and offset-aware TopN for the declared bounded input. Fully parallel ordered result streaming without a coordinator bottleneck and broader pressure/endurance qualification remain open.