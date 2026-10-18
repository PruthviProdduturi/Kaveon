# Join spill skew telemetry — 2026-10-18

Kaveon now emits a bounded join-spill skew signal with each finished task. The
worker records total rows across non-empty spill partitions in addition to the
existing minimum and maximum partition sizes. The API exposes
`join_spill_skew_ratio`, defined as the largest observed non-empty partition
size divided by the mean non-empty partition size.

The spill path now has one bounded skew-aware routing policy. When a spill
level has at least two non-empty buckets, at least 4,096 rows, and the largest
bucket is at least four times the exact non-empty mean, that bucket is split
once with the next level's salt on both join inputs. The split uses only
writer-captured row counts and the same key hash on both sides, so it cannot
change equality semantics. A single non-empty bucket is deliberately excluded:
equal hot keys cannot be separated by another hash, and the existing depth cap
still fails closed. The policy is bounded by `MAX_JOIN_SPILL_DEPTH` and does
not apply at the cap.

The ratio remains diagnostic telemetry for every spill level. A value near
`1.0` indicates balanced partitions; larger values identify the policy's exact
candidate. No non-empty spill partitions means the field is omitted.

Local qualification: `cargo test -p kaveon-exec
skew_policy_uses_exact_run_counts_and_excludes_single_hot_buckets` passed on
the current source tree. The existing `skewed_join_fails_closed` regression
also confirms that a single hot key still reaches the bounded depth failure and
cleans its spill files. The broader exec suite is currently gated by an
unrelated in-flight Decimal256 join change in the shared worktree.

This qualifies the local policy decision and its fail-closed boundary; it does
not qualify AKS high-cardinality behavior, physical-disk exhaustion, or
sustained mixed-workload performance. Those remain open.
