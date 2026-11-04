# Adaptive join fanout — 2026-11-03

## Boundary

When join spill telemetry identifies a multi-key hot bucket, the secondary
split now chooses a deterministic fanout from the encoded bytes written by
both join inputs and the active query memory limit. It preserves the planner's
base partition count, scales up only as needed, and clamps the secondary
fanout at 256. Both sides use the same fanout and salt, so equality-key
alignment and join semantics are unchanged.

The policy does not attempt to separate a single hot key. Such a bucket stays
on the existing depth-capped, fail-closed path. The byte estimate is a routing
signal, not a guarantee of balanced keys or a deployed latency improvement.

## Evidence

- `adaptive_join_fanout_scales_with_hot_bucket_bytes_and_is_bounded` verifies
  minimum, scaling, and hard-cap behavior.
- `skew_split_keeps_both_join_inputs_hash_aligned` verifies exact alignment of
  the two input sides after a secondary split.
- `a_skewed_partition_repartitions_to_the_depth_limit_and_names_it` verifies
  the existing depth cap and fail-closed error remain intact.
- The full server differential sweep
  `the_differential_sweep_matches_across_parquet_encodings_and_execution_paths`
  completed with `1 passed; 0 failed; 707 filtered` in 683.46 seconds after
  this change.

## Qualification status

This qualifies the bounded local adaptive secondary-split policy. Larger
high-cardinality joins, mixed workloads, object-store spill exhaustion, and
deployed/cloud performance qualification remain open.
