# Track 1 AKS: runtime-filter-eligible join probe

## Result

The immutable Engine image `sha256:2c43a790ca59cc39155e53a8d4513c5a713b8c4e9566c336fa8357b4ea1c85fc` (commit `00c46d59`) ran a small-build/large-probe equi-join twice on the three-worker AKS topology. Both statements finished successfully, used all three workers, and produced identical result hashes.

The query joins `OpenSource.public.kaveon_events_enriched` to the small `kaveon_events_dashboard` relation on `region`, then performs a grouped aggregate. The raw report retains the pinned catalog snapshot, query IDs, and per-task scan telemetry. Each run accepted three runtime filters carrying four build-side values each (six accepted filters and 24 values across the two runs).

The raw report is retained in
`track1-aks-runtime-filter-join-current-2026-10-16.json`.

## Boundary

This is an exact distributed probe for a runtime-filter-eligible join shape and proves accepted build-side filters on the deployed Delta path. It does not claim file or row-group reduction: this dataset opened the same files and selected the same row groups after the four-value filter. Dynamic-filter type/exchange edge cases, a dataset that demonstrates measurable storage pruning, and broader cloud snapshot qualification remain open in the parity tracker.
