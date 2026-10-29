# Persisted numeric histograms: qualification

## Scope

Full `ANALYZE` profiles for numeric and temporal columns now persist a bounded
16-bucket equi-depth `NumericHistogram` beside the existing KLL quantile
sketch. The histogram is source-version-bound through `TableStatistics`; it is
not a second scan and it is not an exact frequency table. Its rank error is
recorded from the KLL profile.

Metadata-only profiles and non-numeric columns intentionally keep the
histogram absent. Incremental statistics refreshes regenerate the histogram
after compatible quantile sketches merge; a refresh that cannot preserve the
sketches drops both summaries rather than mixing source versions.

## Evidence

- Core regression verifies bounded bucket count, monotonic boundaries, counts
  summing to the sketch population, numeric range estimation, and JSON
  round-trip: `statistics::tests::numeric_histogram_is_bounded_serializable_and_estimates_ranges`.
- Storage qualification verifies a full Parquet statistics build persists a
  valid histogram and that the complete `TableStatistics` document round-trips
  through JSON: `table_statistics::tests::metadata_and_full_statistics_over_a_parquet_directory`.
- Optimizer selectivity tests exercise the histogram-backed range path and
  preserve the expected selectivity for bounded ranges, nulls, equality and
  disjunctions.
- `cargo clippy -p kaveon-core -p kaveon-storage -p kaveon-optim --all-targets -- -D warnings`
  and `cargo check -p kaveon-server` pass.

## Contract

Histogram estimates are approximate planner inputs. They do not answer SQL
results, replace exact bounds, or claim exact value frequencies. The planner
falls back to KLL rank estimation and then bounds interpolation whenever the
histogram is absent or invalid.

