# Join spill skew telemetry — 2026-10-18

Kaveon now emits a bounded join-spill skew signal with each finished task. The
worker records total rows across non-empty spill partitions in addition to the
existing minimum and maximum partition sizes. The API exposes
`join_spill_skew_ratio`, defined as the largest observed non-empty partition
size divided by the mean non-empty partition size.

This is diagnostic telemetry only. It does not alter partitioning, join
semantics, or admission decisions. A value near `1.0` indicates balanced
partitions; larger values identify a candidate for future skew-aware
repartitioning. No non-empty spill partitions means the field is omitted.

Local qualification: `cargo test -p kaveon-exec skewed_join_fails_closed` and
`cargo test -p kaveon-exec --quiet` passed on the current source tree.

The telemetry does not qualify a skew mitigation policy. AKS high-cardinality,
physical-disk exhaustion, and sustained mixed-workload qualification remain
open.
