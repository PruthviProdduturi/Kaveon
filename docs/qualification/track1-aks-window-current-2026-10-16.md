# Track 1/2 AKS: current-image partitioned window qualification

## Result

The immutable Engine image `sha256:60de4cf526c2340959fcd0ce264c9c4cca1680ced8d462617c4287d93e01f8c9` ran a partitioned `ROW_NUMBER()` window with a `QUALIFY` predicate twice on the three-worker AKS topology.

Both runs finished successfully with three stages, seven tasks, all three workers, and the same 12-row result hash (`de1f2aefd6a2526076599441d11a9f18585102e57d213671d46f20065604469e`). The query ranks the top two actions per region after filtering the 504M-row source to a bounded date/user slice.

The raw report is retained in
`track1-aks-window-current-2026-10-16.json`.

## Boundary

This qualifies the declared partitioned `ROW_NUMBER()` plus `QUALIFY` shape on the current AKS image. Unpartitioned windows, additional frames/modifiers, broader ranking/value-window coverage, and fully parallel execution for remaining window shapes remain open in both parity tracks.