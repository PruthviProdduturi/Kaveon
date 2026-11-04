# Adaptive final-merge fanout — 2026-11-03

## Boundary

The final grouped-aggregate merge previously used a fixed 16-way recursive
repartition whenever an encoded spill bucket exceeded the active memory
budget. This change makes the fanout deterministic but data-aware: it sums
the published Arrow spill-run bytes, targets at most one quarter of the
active query memory limit per child, and clamps the result to 16–256 ways.
The policy is applied only after a merge has already reached the bounded spill
path; it does not change ordinary in-memory aggregation or the exchange
partition count.

The byte estimate is an upper-bound planning signal for the repartition step,
not a promise that every child is balanced. Equal or adversarial keys can
still remain skewed, so the existing recursive depth cap and fail-closed
memory error remain in force.

## Evidence

- `adaptive_repartition_factor_scales_with_encoded_spill_bytes` verifies the
  minimum fanout, scaling for a larger encoded run, and the hard 256-way cap.
- `hybrid_merge_fails_closed_when_a_sub_partition_does_not_fit` verifies that
  the bounded recursive path still records repartition depth and releases all
  memory and spill reservations when a single-key skew cannot be separated.
- `cargo test -p kaveon-exec --lib` remains the required full executor gate;
  the focused tests pass on the current source tree.

## Qualification status

This qualifies the adaptive fanout policy for the local bounded-memory final
merge path. It does **not** close the broader distributed aggregate gap:
large-scale workload latency, cloud/object-store endurance, spill-device
exhaustion, and adaptive exchange behavior across deployed worker topologies
still require workload and deployment evidence.
