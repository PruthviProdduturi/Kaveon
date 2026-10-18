# Track 1 — encoded-byte join skew policy qualification

## Scope

The bounded secondary join split now uses two independent exact signals from
spill-run writer facts: the existing row-count ratio and encoded-byte ratio.
The byte path requires at least 4 MiB across the non-empty partitions and a
largest partition at least four times the exact non-empty mean. This catches a
wide-key partition whose row count is ordinary while keeping small inputs on
the established path. A single non-empty partition remains excluded because a
new salt cannot separate equal join keys safely.

## Evidence

From `engine/`:

```text
cargo test -p kaveon-exec partitioned::tests::skew_policy -- --nocapture
running 2 tests
test partitioned::tests::skew_policy_uses_exact_run_counts_and_excludes_single_hot_buckets ... ok
test partitioned::tests::skew_policy_splits_a_wide_partition_by_encoded_bytes ... ok
test result: ok. 2 passed; 0 failed

cargo test -p kaveon-exec partitioned::tests::skew_split_keeps_both_join_inputs_hash_aligned -- --nocapture
running 1 test
test partitioned::tests::skew_split_keeps_both_join_inputs_hash_aligned ... ok
test result: ok. 1 passed; 0 failed
```

The byte-skew regression writes eight non-empty partitions with equal row
counts. One partition contains deterministic low-compressibility 10,000-byte
strings and the other seven contain one-byte strings. The test therefore
requires the decision to come from encoded bytes, while the alignment test
continues to prove that the secondary salt is applied consistently to both
join inputs.

## Boundary

This is local exact spill-policy evidence. It does not qualify high-cardinality
skew on AKS, sustained mixed workload behavior, or a general cost-based join
enumerator. The policy remains bounded and falls back to the existing
depth-capped fail-closed path when neither exact signal qualifies.
