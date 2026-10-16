# Track 1 AKS: runtime-filter-eligible join probe

## Result

The immutable Engine image `sha256:60de4cf526c2340959fcd0ce264c9c4cca1680ced8d462617c4287d93e01f8c9` ran a small-build/large-probe equi-join twice on the three-worker AKS topology. Both statements finished successfully, used all three workers, and produced identical result hashes.

The query joins `OpenSource.public.kaveon_events_enriched` to the small `kaveon_events_dashboard` relation on `region`, then performs a grouped aggregate. The raw report retains the pinned catalog snapshot and query IDs.

The raw report is retained in
`track1-aks-runtime-filter-join-current-2026-10-16.json`.

## Boundary

This is an exact distributed probe for a runtime-filter-eligible join shape. The report does not claim scan pruning: the current API history does not expose a dedicated runtime-filter application counter for this query. Dynamic-filter type/exchange edge cases, storage-pruning counters, and cloud snapshot qualification remain open in the parity tracker.