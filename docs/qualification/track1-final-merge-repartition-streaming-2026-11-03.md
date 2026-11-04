# Final-merge repartition streaming qualification — 2026-11-03

## Boundary

When a high-cardinality grouped aggregate must recursively repartition a
spilled final-merge bucket, Kaveon now streams the encoded key and aggregate
state values directly into per-partition Arrow binary builders. The path no
longer constructs a row-index vector and materializes a `take` batch for every
partition. Partition hashing, binary schemas, NULL state values, spill-run
publication, cancellation accounting, and the existing repartition-depth cap
remain unchanged.

Broadcast final merges also divide their prepaid input-batch admission across
the selected sibling workers. A worker no longer reserves four complete input
batches on a shared query budget when two other workers are making the same
reservation; this leaves room for the bounded spill path to respond to
pressure instead of failing before it can spill.

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
- Release pressure benchmark:
  `cargo test --release -p kaveon-exec final_merge::tests::final_merge_under_pressure -- --ignored --nocapture`
  passed all 18 tight/roomy partitioned, broadcast, and source-reserving
  rounds on 2026-11-03. The tight broadcast rounds completed in 1.32–1.42 s
  with 9 source spill tables and no memory-limit failure; the prior four-batch
  per-worker admission failed before the first tight broadcast round.
- Formatting and focused execution tests passed on 2026-11-03.

## Remaining gate

Measure allocation and CPU deltas on representative high-cardinality grouped
workloads, then repeat the same correctness and cleanup checks on the release
image. The broader adaptive aggregate-exchange item remains open.
