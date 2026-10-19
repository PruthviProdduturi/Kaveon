# Global TopN multi-key Decimal128 heap qualification

## Scope and result

The sorted, non-null multi-key heap merge now accepts `Decimal128` alongside
`Int64` and `UInt64`. It compares exact unscaled `i128` values without a
floating-point conversion. A key position retains the same precision and
scale across input batches; different key columns can have different scales.
No scale conversion is introduced. Nullable keys, unsupported types, and
unsorted candidates retain the existing Arrow comparator/lexsort paths.

The existing heap keeps one head per contributing worker, requiring O(P)
heap construction and O(K log P) head selection for P batches and K output
rows. Equal complete keys retain source-batch order and within-batch order.
Key evaluation, copied key values, sortedness validation, and payload
materialization retain their existing costs. This is bounded in-process
correctness qualification, not a deployed throughput or pressure claim.

## Evidence

Commands ran from `engine/` on 2026-10-18:

```text
cargo test -p kaveon-exec topn::tests --quiet
31 passed; 0 failed

cargo test -p kaveon-server distributed_global_topn_multi_decimal_heap_matches_local -- --nocapture
1 passed; 0 failed

cargo clippy -p kaveon-exec -p kaveon-server --all-targets -- -D warnings
Passed

git diff --check
Passed
```

The executor tests call the specialization directly, so an Arrow fallback
cannot hide failure to select decimal candidates. They compare every output
column against Arrow `lexsort_to_indices` over 33 uneven worker batches,
with Decimal128 precision 38 and scales 6 and 2. They exercise all four
direction combinations, both signed precision-38 extrema, adjacent large
values that floating point cannot distinguish, primary-key ties, worker
exhaustion, LIMIT 1, full-size and oversized limits, and zero-limit dispatch.
A separate 17-worker fixture checks complete-key ties with mixed decimal and
`UInt64` keys (including `u64::MAX`) and a limit cutting through the peer set.
Another regression verifies nullable and unsorted inputs reject the typed
specialization and match Arrow for both directions and null placements.

The partitioned regression writes 528 rows into 33 Parquet files. It compares
node-local planning with the existing fragment/exchange executor using 3,
17, and 40 workers, `LIMIT 73 OFFSET 19`, all four decimal direction
combinations, and both null placements. Every case runs both with nullable
keys and with an explicit non-null filter to exercise the typed heap.
All 48 distributed results equal the local Arrow record batch exactly,
including decimal precision/scale, raw values, payload order, and row count.
Every run releases its query memory reservations. Forty workers also cover
tasks with no assigned files. These tests do not invoke a live Trino service.

## Remaining gate

Track 1 item 6 remains open for parallel ordered output and sustained
pressure behavior without a coordinator bottleneck. Decimal256 multi-key
typed heap selection, cross-scale conversion within a key, cloud/deployed
qualification, and sustained distributed ordering remain outside this
bounded change.
