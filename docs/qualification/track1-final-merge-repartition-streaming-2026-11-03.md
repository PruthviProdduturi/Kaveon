# Final-merge repartition streaming qualification — 2026-11-03

## Boundary

When a high-cardinality grouped aggregate must recursively repartition a
spilled final-merge bucket, Kaveon now streams the encoded key and aggregate
state values directly into per-partition Arrow binary builders. The path no
longer constructs a row-index vector and materializes a `take` batch for every
partition. Partition hashing, binary schemas, NULL state values, spill-run
publication, cancellation accounting, and the existing repartition-depth cap
remain unchanged.

This is a bounded materialization optimization. It does not claim that all
high-cardinality aggregate exchange is adaptive, nor does it qualify a
universal latency or allocation improvement on a deployed image.

## Evidence

- Implementation: `engine/crates/exec/src/final_merge.rs`,
  `HybridFinalMerge::repartition_runs`.
- Exact skew/depth-cap regression:
  `final_merge::tests::hybrid_merge_fails_closed_when_a_sub_partition_does_not_fit`.
- Exact duplicate DISTINCT spill/replay regression:
  `final_merge::tests::hybrid_merge_spill_unions_duplicate_distinct_values_across_runs`.
- Formatting and focused execution tests passed on 2026-11-03.

## Remaining gate

Measure allocation and CPU deltas on representative high-cardinality grouped
workloads, then repeat the same correctness and cleanup checks on the release
image. The broader adaptive aggregate-exchange item remains open.
