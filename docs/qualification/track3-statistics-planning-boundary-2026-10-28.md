# Statistics and planning boundary qualification — 2026-10-28

## Qualified local contract

Kaveon's statistics pipeline persists immutable, source-version-bound facts for
the supported Parquet, Delta, and Iceberg readers. The qualified facts include
exact row counts and bytes, null counts, min/max bounds, HyperLogLog distinct
estimates, and KLL quantiles. Statistics are never used to answer a query when
their catalog or source identity is stale.

The optimizer consumes those facts for bounded local planning: equality and
range selectivity, filtered-cardinality estimates, inner-join ordering, and
broadcast admission. File and row-group readers also use compatible bounds
and runtime filters for pruning where the format reader exposes them.

## Evidence

- `kaveon_storage::table_statistics::tests::metadata_and_full_statistics_over_a_parquet_directory`
- `kaveon_storage::table_statistics::tests::delta_statistics_come_from_the_log_and_refresh_by_version`
- `kaveon_optim::statistics::tests::selectivity_follows_distinct_counts_bounds_nulls_and_quantiles`
- `kaveon_optim::statistics::tests::broadcasts_only_a_proven_small_materially_smaller_inner_build`
- `kaveon_optim::statistics::tests::exact_source_broadcast_bounds_ignore_stale_and_incomplete_documents`
- `kaveon_optim::statistics::tests::a_filtered_scan_with_statistics_becomes_the_build_side_by_estimated_rows`
- `kaveon_optim::statistics::tests::a_limited_build_relation_uses_its_cardinality_bound_for_broadcast`

## Boundary

This does not claim a persisted histogram model, a universal cost-based
optimizer, or measurable cloud row-group pruning for every source. Those remain
separate qualification gates, as do plan-vs-runtime regression thresholds and
cross-coordinator snapshot behavior.
