# Track 1 — local join skew policy qualification

## Scope

The bounded join skew policy uses exact spill-run row counts. A multi-key hot bucket is eligible for one secondary salted split only when it has at least 4,096 rows and is at least four times the exact non-empty mean. A single non-empty bucket remains on the depth-capped fail-closed path because equal keys cannot be separated safely by another hash salt.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec skew_policy_uses_exact_run_counts_and_excludes_single_hot_buckets --quiet
running 1 test
.\ntest result: ok. 1 passed; 0 failed

cargo test -p kaveon-exec skew_split_keeps_both_join_inputs_hash_aligned --quiet
running 1 test
.\ntest result: ok. 1 passed; 0 failed

cargo test -p kaveon-exec skewed_join_fails_closed_and_cleans_all_partitions --quiet
running 1 test
.\ntest result: ok. 1 passed; 0 failed
```

The tests cover exact ratio/minimum-row decisions, aligned salted repartitioning of both join inputs, and cleanup after a single-hot-key memory refusal.

## Boundary

This is local spill-policy evidence. AKS qualification under high-cardinality skew and sustained mixed workload remains open.