# Global TopN multi-key UTF-8 heap qualification

## Scope and result

The sorted, non-null multi-key heap merge now accepts UTF-8 alongside
`Int64`, `UInt64`, and `Decimal128`. It compares string slices using Arrow's
binary ordering, retaining shared Arrow string buffers rather than copying
every key string or building a per-worker comparator matrix. Mixed string
and exact numeric ordering keys use the same heap. Nullable keys,
unsupported types, and unsorted candidates retain the existing Arrow paths.

The heap keeps one head per contributing worker, with O(P) heap construction
and O(K log P) head selection for P batches and K output rows. String
comparison still depends on the common-prefix length; input key evaluation,
sortedness validation, numeric key copies, and final payload materialization
retain their existing costs. Complete-key ties retain source-batch and
within-batch order. This is bounded in-process correctness evidence, not a
deployed throughput or pressure claim.

## Evidence

Commands ran from `engine/` on 2026-10-18:

```text
cargo test -p kaveon-exec topn::tests --quiet
33 passed; 0 failed

cargo test -p kaveon-server distributed_global_topn_multi_utf8_heap_matches_local -- --nocapture
1 passed; 0 failed

cargo clippy -p kaveon-exec -p kaveon-server --all-targets -- -D warnings
Passed

git diff --check
Passed
```

The executor regression directly calls the specialization so a generic
fallback cannot hide a missing UTF-8 fast path. It compares every output
column against Arrow `lexsort_to_indices` over 33 uneven worker batches.
It covers empty strings, embedded NUL, prefixes, long common prefixes,
composed and decomposed Unicode, multibyte characters and emoji, sliced
arrays with nonzero offsets, independent string directions, both null
placements, mixed `UInt64` keys near `u64::MAX`, worker exhaustion, LIMIT 1,
full and oversized limits, and zero-limit dispatch. Nullable and unsorted
candidates explicitly reject typed selection and retain exact Arrow results.
A 17-worker UTF-8/Decimal128 fixture checks complete-key ties and a limit
cutting through the peer set, preserving exact precision-38 decimal values
and payload order.

The partitioned regression writes 528 rows into 33 Parquet files. It compares
node-local planning against the fragment/exchange executor with 3, 17, and 40
workers, LIMIT 73 OFFSET 19, all four string direction combinations, and
both null placements. Each shape runs with nullable keys and with explicit
non-null filters. All 48 distributed outputs equal the local Arrow record
batch exactly, including string bytes, payload order, schema, and row count.
Each run releases all query memory reservations; 40 workers also cover tasks
without assigned files. No live Trino service is invoked.

## Remaining gate

Track 1 item 6 remains open for parallel ordered result streaming and
sustained pressure without a coordinator bottleneck. Nullable typed heap
selection, `LargeUtf8`/dictionary/UTF-8 view specialization, and deployed
throughput or sustained distributed ordering are outside this bounded change.
