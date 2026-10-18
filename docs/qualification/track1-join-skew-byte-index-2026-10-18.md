# Track 1 join skew: byte-largest bucket selection

## Scope

The bounded secondary split policy uses exact spill run facts to identify one
hot partition. A partition can be hot by encoded bytes while having an ordinary
row count, so row and byte skew must select their respective largest buckets.

## Result

The selector now tracks the row-largest and byte-largest non-empty partitions
independently. Row skew continues to route the row-largest bucket; byte skew
routes the byte-largest bucket. This prevents a wide string-key bucket from
being missed when another partition happens to contain slightly more rows.

Focused qualification:

```text
cargo test -p kaveon-exec skew_policy_
2 passed; 0 failed
```

The regression extends `skew_policy_splits_a_wide_partition_by_encoded_bytes`
with a byte-hot bucket at a different index from the row-largest bucket. The
existing multi-key skew, single-hot-bucket, and small-input guards remain green.

## Boundary

This fixes selection of the bounded secondary split only. It does not claim
that a single hot join key can be separated, nor does it qualify high-cardinality
skew behavior on AKS. Those remain deployment and workload gates.
