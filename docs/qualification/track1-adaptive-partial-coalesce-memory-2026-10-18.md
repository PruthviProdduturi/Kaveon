# Adaptive partial coalescing memory contract

Date: 2026-10-18

High-cardinality adaptive partial aggregation can retain several encoded
pass-through batches before concatenating them for the exchange. The
coalescing path now reserves each retained encoded batch while it is held and
releases those temporary reservations only after concatenation. This keeps
the query memory pool aware of retained input arrays during the peak
allocation; the returned batch remains owned by the downstream output account.

The focused regression enables a 16-row coalescing window for 300,000 unique
keys and checks exact group/count/sum/max results, adaptive pass-through
metrics, and zero current bytes after draining:

```text
cargo test -p kaveon-exec a_partial_over_near_unique_keys_passes_rows_through_and_merges_exactly -- --nocapture
PASS (1 test)
```

The implementation adds an explicit `with_coalesce_rows` testable setting;
the production environment setting remains opt-in. This validates bounded
memory accounting for the coalescing path, not universal aggregate spill or
all high-cardinality distributed workloads.
